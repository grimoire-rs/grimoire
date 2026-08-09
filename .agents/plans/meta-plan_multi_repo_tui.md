# Meta-Plan: Multi-Repo TUI (Resolves #16)

> Preview of the planning run. Not the plan itself. Approve to produce the
> real plan artifact.

## GitHub Context

- **Issue #16** — "multi repo tui" / body: "Support multiple repos in tui"
- Labels: none. Comments: none. Author: owner.
- Terse target → **low-confidence classification** → this gate fires.

## Classification (recommended)

- **Scope:** Medium (single subsystem `src/tui/**` + entry seam `src/command/tui.rs`; ~5–7 files)
- **Reversibility:** Two-Way Door — UI-internal extension. No public API, no
  protocol, no storage/index format change. Catalog cache already per-registry.
- **Tier:** `high` (recommended) — multi-file TUI change with real internal
  design decisions, but not a one-way door.
- **Overlays:** architect=`inline` (Two-Way), research=`skip` (infra + framework
  already in place), codex=`off`.

## Key Discovery (already done by 2 explorers)

Multi-registry infra **already shipped** below the TUI:
- `catalog_service::load_catalog()` returns `CatalogResults { groups: Vec<CatalogGroup> }`
  (one group per registry, parallel-loaded, registry-grouped). Used by `grim search` + MCP.
- Config `[[registries]]` array, `resolve_registries()`, alias-qualified refs,
  per-registry docker credentials — all shipped.
- `TuiRow` already carries a `registry` field; rows already registry-qualified.
- TUI tree infra (`src/tui/tree.rs`, `view_mode`, `collapsed`, `group_by_type`)
  already exists from `plan_tui_tree_view.md`.

**The TUI is the only single-registry holdout.** `command/tui.rs` resolves ONE
registry; `TuiContext` holds `registry: String` + single `catalog_path`. Comment
at `app.rs:866` already flags "collapsible registry tree is a deferred follow-up".
This issue IS that follow-up.

## Work Shape (what the plan will cover)

1. **Entry seam** `command/tui.rs`: resolve full registry set via `registries_for_scope()`
   (mirror `grim search`), call `load_catalog()` instead of single-registry path.
2. **TuiContext** (`app.rs`): hold registry set + `CatalogResults` groups instead of
   single `registry` + `catalog_path`. Preserve scope-swap (Project⇄Global) orthogonal
   to registry set.
3. **State/tree** (`state.rs`, `tree.rs`): promote registry host to a top-level tree
   group (analogous to `group_by_type`); registry roots collapsible.
4. **Render** (`render.rs`): registry-root rows in `RenderModel`; host-elision logic
   (`default_registry`) adapts to multi-registry (per-group or disabled).
5. **Invariants to preserve:** selection anchors to leaf identity across registries;
   marked-set stable across view/filter; bundle-member cache key extended with registry.
6. **Degradation:** per-registry load failure degrades that group to empty (others
   continue) — already the catalog-layer contract; surface in TUI status line.

## Workers I Would Launch (tier=high)

| Phase | Worker | Count | Role |
|---|---|---|---|
| Discover | (done) `Explore` ×2 | — | TUI map + registry model (complete) |
| Discover | `worker-explorer` | 1 | Deep-read `tree.rs` + tree-view plan for projection reuse |
| Research | — | 0 | Skipped (infra + ratatui already in repo) |
| Design | inline (orchestrator) | — | Plan artifact w/ contracts + UX scenarios |
| Review | `worker-reviewer` (spec-compliance) | 1 | Plan consistency, 2 rounds |

## Artifacts I Would Produce

- `.agents/plans/plan_multi_repo_tui.md` (with `## Status` block)
- `.claude/state/current_plan.md` (pointer)
- No `research_*.md` (research skipped)
- No ADR (Two-Way Door)

## Estimated Cost

- Parallel workers: ≤2 at peak (1 explorer + 1 reviewer, sequenced)
- Heaviest call: inline design synthesis (orchestrator)
- Codex plan review: off

## Not Doing

- No implementation (that's `/swarm-execute`)
- No PR creation, no push
- No new infra (catalog multi-registry already shipped)
- No registry config schema change
