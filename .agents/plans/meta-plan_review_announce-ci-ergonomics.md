# Meta-Plan: /swarm-review max — announce CI ergonomics

## Classification

- **Tier:** max (confident)
- **Rationale:** 16 files / +742 −86 (828 lines) across 2 subsystems (`src/**`, `test/**`); **breaking JSON contract** (publish `--format json` bare-array → wrapper object, One-Way Door); **credential/token injection** into git subprocess (security-sensitive auth path). Three independent max signals converge.
- **Confidence:** high — no adjacent-tier competition.

## Baseline

- `main` (default; long-lived feature branch `feat/announce-ci-ergonomics`, 3 commits ahead).

## Target

- `HEAD` (branch `feat/announce-ci-ergonomics`).

## Overlays (tier-max defaults, no user override)

- **breadth:** adversarial
- **reviewer:** opus (adversarial breadth escalates sonnet→opus)
- **doc-reviewer:** sonnet (5 `docs/src/*.md` touched → narrow-scope haiku trigger does NOT fire)
- **rca:** on (all findings above Suggest)
- **codex:** on (mandatory cross-model gate)

## Workers

**Stage 1 — Correctness (2 parallel):**
- `worker-reviewer` spec-compliance (post-implementation traceability)
- `worker-reviewer` quality/test-coverage (announce outcome + credential matrix + HOME-less coverage)

**Stage 2 — Adversarial (6 parallel):**
- `worker-reviewer` quality (+ CLI-UX lens — command surface touched)
- `worker-reviewer` security (**primary focus: credential-helper injection, token leak surface, MR-API ban invariant**)
- `worker-reviewer` performance
- `worker-doc-reviewer` (6 docs + CLAUDE.md + ADR + rule drift vs code)
- `worker-architect` (SOLID/boundary/dep-direction; diff vs ADR D6 amendment)
- `worker-researcher` (SOTA: CI-token git-push credential patterns; publish JSON wrapper-vs-array convention)

**Cross-model:** `codex-adversary` scope `code-diff --base main` (mandatory).

## Estimated cost

8 Claude workers (opus reviewers) + 1 Codex pass. Highest-cost tier. Read-only.

## Not Doing

- No auto-fixes, no commits, no pushes — review is read-only. Findings reported ranked, classified actionable/deferred.
