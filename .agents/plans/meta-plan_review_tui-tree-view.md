# Meta-Plan: /swarm-review — TUI Tree View (post-remediation gate)

> Single approval point. Max tier auto-fires this gate (cost transparency).
> On approve → dispatch `tier-max.md`. No code touched until then.

## Context: this is a re-review

- Prior `/swarm-review` (max) ran against **b5a6d24** → Request Changes (A1 Block + Warn cluster).
- `/swarm-execute high` ran the Review-Fix Loop → fixed in **f37fa22**; round-2 reviewers returned PASS / 0 actionable.
- This run is the **independent adversarial gate on the converged result** (b5a6d24 + f37fa22) — the normal step between `/swarm-execute` and `/finalize`. The earlier review reviewed pre-remediation code; this reviews what will actually land.

## Classification

| Axis | Value | Source |
|---|---|---|
| Tier | **max** | auto — 18 files, +3160/−50, cross-subsystem |
| file_count | 18 | `git diff main...HEAD --name-only` |
| lines_changed | 3210 (+3160 / −50) | `--shortstat` |
| subsystems | **5** — `src/tui`, `src/config`, `src/command`, `docs`, `test` (+ artifacts) | rules.md "By subsystem" |
| structural markers | none One-Way-Door (pure projection, no new crate, no public API break, no network) | classify.md |
| confidence | **high** (file_count >15 AND ≥2 subsystems both fire max; no competing adjacent signal) | — |

Metrics dictate max on size + cross-subsystem alone. No One-Way-Door marker, but the size/subsystem rule is sufficient and unambiguous.

## Baseline

- **main** (default — no `--base` supplied). Yields full feature diff (both commits).
- Cost-conscious alternative the user may prefer: `--base=b5a6d24` reviews **only** the f37fa22 remediation (~9 files) → would reclassify to **high**, drop codex/architect/SOTA. Offered at the gate.

## Overlays (max defaults; no user flags)

| Axis | Value | Rationale |
|---|---|---|
| breadth | **adversarial** | tier=max default — adds `worker-architect` + `worker-researcher` (SOTA) + CLI-UX lens |
| reviewer | **opus** | tier=max + adversarial breadth (overlays.md reviewer axis) |
| doc-reviewer | **sonnet** | diff touches 2 doc pages + CHANGELOG; primary user guide (`docs/src/configuration.md`, `commands.md`) touched → not narrow-scope, stays sonnet |
| rca | **on** (all findings > Suggest) | tier=max default |
| codex | **on (mandatory)** | tier=max final cross-model gate |

## Workers per perspective

**Stage 1 — Correctness (2 parallel):**
- `worker-reviewer` (spec-compliance, post-implementation) — opus
- `worker-reviewer` (quality, lens: test-coverage) — opus

**Stage 2 — Adversarial panel (up to 6 parallel):**
- `worker-reviewer` (quality + CLI-UX lens) — opus
- `worker-reviewer` (security) — opus
- `worker-reviewer` (performance) — opus
- `worker-doc-reviewer` — sonnet
- `worker-architect` (SOLID, boundary, dep direction, projection-over-index ADR check) — opus
- `worker-researcher` (SOTA: how lazygit / k9s / gitui / fzf-tree do tree+filter+multi-select) — sonnet

**Phase 4:** RCA (Five Whys) on every Block/High/Warn; cluster by root.
**Phase 5:** `codex-adversary` scope `code-diff --base main`, one-shot, read-only triage.

Concurrency: Stage 1 (2) then Stage 2 (6) — ≤8 ceiling respected.

## Estimated cost

- 8 worker agents (6 opus, 2 sonnet) + 1 Codex pass. High token spend.
- This is why the gate exists. If the user trusts the converged internal loop, the **high-scoped** (`--base=b5a6d24`) or **skip-to-/finalize** options below cost far less.

## Not Doing

- No auto-fixes (review is read-only — findings reported, not committed).
- No commits, no push.
- No re-running the `/swarm-execute` Review-Fix Loop (handoff only, on the user's call).
- Out of scope: Phase 2 bundle membership; registry-config dedup ADR (separate workstream).
