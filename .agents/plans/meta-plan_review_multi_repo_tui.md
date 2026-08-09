# Meta-Plan — /swarm-review: multi-registry TUI

## Classification

- **Target:** `HEAD` (branch `feat/multi-repo-tui`), 3 commits ahead of `main`
- **Baseline:** `main` (default — no `--base` given)
- **Diff metrics:** 14 files, +1987 / −227
  - **src (real review scope):** 7 files, +1483 / −206 = **1689 lines**
    (`src/tui/{tree,app,render,state,event,update_check}.rs`, `src/command/tui.rs`)
  - test: 2 files, +463 (new `test_tui_multi_registry.py` 417 + manual README 46)
  - docs/catalog/.claude: 5 files, +41 / −21
- **Subsystems:** src/tui (core), src/command, test, docs, catalog, .claude
- **Reversibility:** Two-Way Door — internal TUI projection; no on-disk format,
  no public API, no protocol, no new crate. Fully reversible.
- **Candidate tier:** borderline. Line count (1689 src) exceeds high's ≤500 cap
  → **max** by raw metric. But file_count 14 ≤15, one dominant code subsystem,
  Two-Way Door, and already reviewed at **high** during `/swarm-execute`
  (3-round Review-Fix Loop: spec / quality / security / perf / docs all Pass,
  0 actionable; B1 status-line + B1 member-registry fixes landed).
  → **low-confidence (high↔max split)** → this gate.

## Recommendation: tier=high, full breadth

Independent adversarial re-review at **high** re-runs every meaningful
perspective fresh against the diff, without max's heavyweight extras
(SOTA researcher, adversarial architect, mandatory Codex) that add little
for a reversible, already-high-reviewed internal feature.

- **Breadth:** full
  - Stage 1: spec-compliance + test-coverage (always)
  - Stage 2: quality + security + performance + documentation
- **reviewer model:** sonnet (high default)
- **doc-reviewer model:** sonnet — `docs/src/commands.md` is the primary user
  guide, so the narrow-scope haiku trigger does NOT fire
- **rca:** on (Block/High findings)
- **codex:** off — Two-Way Door, no One-Way Door signal (consistent with the
  execute-phase decision)

## Workers

| Stage | Workers (parallel) |
|---|---|
| Stage 1 | `worker-reviewer` spec-compliance (post-implementation) · `worker-reviewer` quality (lens: test-coverage) |
| Stage 2 | `worker-reviewer` quality · `worker-reviewer` security · `worker-reviewer` performance · `worker-doc-reviewer` |

Max 8 concurrent. Up to 3 rounds (high); re-run only perspectives with
actionable findings each subsequent round.

## Alternative: tier=max

Escalate if you want the full adversarial pass: + `worker-architect`
(SOLID/boundary/dependency-direction, adversarial), + `worker-researcher`
(SOTA gap check), + CLI-UX lens, reviewer→opus, rca for all >Suggest, and
**mandatory Codex** cross-model code-diff gate. Heavier; marginal for a
reversible feature already reviewed at high.

## Not Doing

- NO auto-fixing — review is read-only; actionable findings reported, not committed
- NO commits, NO push
- Stays within diff scope (`main...HEAD`)
