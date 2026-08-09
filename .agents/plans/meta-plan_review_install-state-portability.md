# Meta-Plan: /swarm-review — install-state-portability

## Classification

| Axis | Value | Source |
|---|---|---|
| Tier | **max** | auto — 28 files, +4950/−350, ≥2 subsystems, security paths |
| Baseline | `main` | default (no `--base`) |
| Target | `HEAD` (branch `feat/install-state-portability`, WIP commit 577caee) | current branch |
| Confidence | high | 28 files ≫ 15 max-threshold; cross-subsystem; new traversal-guard module |

**Diff metrics:** 28 files, +4950 / −350. Subsystems: `src/install/**` (file-structure),
`src/command/**` + `scope_resolution` (cli seam), `src/tui/**`, `test/**`, `docs/**`,
`.claude/rules`. Structural markers firing → max: cross-subsystem; new
`src/install/path_anchor.rs` (path-traversal security guard); V1→V2 serde wire-format
migration (protocol/One-Way-Door signal); plan Reversibility = One-Way-Door Med-High.

## Overlays (max defaults)

| Axis | Value | Source |
|---|---|---|
| breadth | `adversarial` | tier=max default (+ architect + SOTA researcher + CLI-UX lens) |
| reviewer | `opus` | tier=max + adversarial breadth (security floor already ≥ sonnet) |
| doc-reviewer | `sonnet` | diff touches docs/ user guide + >2 doc files (haiku trigger not fired) |
| rca | `on` (all > Suggest) | tier=max default |
| codex | `on` (mandatory) | tier=max default + security/protocol markers |

## Workers per perspective

**Stage 1 — Correctness (2, parallel):**
- `worker-reviewer` spec-compliance (post-implementation traceability) — opus
- `worker-reviewer` quality / test-coverage lens (Specify-phase adequacy) — opus

**Stage 2 — Adversarial panel (6, parallel; 8-worker ceiling):**
- `worker-reviewer` quality + CLI-UX lens — opus
- `worker-reviewer` security (path traversal, TOCTOU, migration safety) — opus
- `worker-reviewer` performance (double-resolve, index, clones) — opus
- `worker-doc-reviewer` (CLAUDE.md, CHANGELOG, configuration.md, subsystem rule) — sonnet
- `worker-architect` (SOLID, boundary, dep direction, ADR compliance) — opus
- `worker-researcher` (SOTA: how Cargo/npm/uv/Helm key machine-local state)

**Phase 4 RCA:** Five Whys on every Block/High/Warn; cluster by root.
**Phase 5 Codex:** `codex-adversary` code-diff `--base main`, one-shot (read-only; no builder fix-pass).

## Estimated cost

~8 parallel agents (opus-heavy) + 1 Codex pass + RCA synthesis. Largest review config.
**Note:** swarm-execute already ran 3 Claude review rounds (50+ findings) + a Codex gate
during implementation. This pass is an independent fresh-eyes review of the converged final
state — marginal value real (independent architect/SOTA/security perspectives) but diminishing.

## Not Doing

- NO auto-fixing (review is read-only — findings reported, not committed)
- NO commits, NO push
- NO re-entering Review-Fix Loop (handoff to `/swarm-execute` if actionable findings)
