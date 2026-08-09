# Meta-Plan: /swarm-execute max — apply review findings (config-list-all)

## Classification

- **Tier:** max — user-forced (`/swarm-execute max ...`). Honest note: work
  itself is fix-pass-small (13 items, 1 behavioral); max retained for the
  mandatory Codex gate + opus builders per tier contract.
- **Plan artifact:** `.agents/plans/plan_review_fixes_config_list_all.md`
  (authored from the `/swarm-review max` verdict — findings are the plan;
  full `/swarm-plan max` pipeline skipped as waste for a fix-pass).
- **Source:** 11 actionable findings (1 High, 3 Warn, 7 Suggest) + Codex
  corroboration.

## Overlays (resolved)

| Axis | Value | Source |
|---|---|---|
| builder | opus (W1, W2) | tier=max mandatory |
| tester | sonnet (W3) | test-only mechanical additions; max's opus-tester mandate applies to greenfield Specify phase, not drift-test backfill — deviation surfaced here |
| doc-writer | sonnet (W4) | worker default |
| loop-rounds | ≤3 | tier default |
| review breadth | Stage 1 + targeted architect | DEVIATION from max adversarial: full panel ran minutes ago on base diff; fix delta small. Approving this meta-plan approves the deviation |
| codex | on, sol, one-shot on amended diff | tier=max mandatory |

## Workers per phase

| Phase | Workers | Parallel |
|---|---|---|
| 1 Fix waves | W1 builder(opus) validation+messages+tests; W2 builder(opus) ResolvedOptions slim; W3 tester(sonnet) drift tests; W4 doc-writer(sonnet) docs | 4, file-disjoint |
| 2 Gate | orchestrator: rust:verify → targeted acceptance → task verify | — |
| 3 Review | reviewer(spec-compliance vs findings list) + reviewer(test-coverage) + architect(ResolvedOptions check) | 3 |
| 4 Codex | codex-adversary code-diff --base main --model sol | 1 |
| 5 Commit | orchestrator, 4 conventional commits | — |

## Test-first gate

W1's load-time validation = the one behavioral change → failing acceptance
tests authored + run red BEFORE fix lands (workflow-bugfix discipline).

## Estimated cost

2 opus + 2 sonnet workers + 3 review agents + Codex ≈ 350–550k tokens.

## Not Doing

- Push / PR creation
- Deferred review findings (wire typing, security hardening scope, catalog
  audit commit, ADR refresh)
- Full Stage 2 adversarial re-panel (deviation above)
