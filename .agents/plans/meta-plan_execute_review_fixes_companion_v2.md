# Meta-Plan: /swarm-execute max — review-fixes companion v2

## Classification

- **Tier:** max (user-explicit). Free-text target bridged: remediation
  plan authored inline from the max-tier review verdict (this session) —
  `.agents/plans/plan_review_fixes_companion_v2.md`. A fresh
  `/swarm-plan max` would only re-derive the same findings list.
- **Scope:** 2 Block + 7 Warn + 9 cheap Suggests across publish/fetch/
  release/oci/api/tests/docs on branch `feat/vscode-extension-api`
  (snapshot bd6f1ee, verify green).

## Embedded policy defaults (approval covers)

1. Containment = **reject** out-of-tree `[description]` paths (65);
   monorepo opt-in flag = Not Doing.
2. `--vendor` + `--description` = **64 gate** (reconciles ADR Risks).

## Overlays (resolved; max mandatories)

| Axis | Value | Source |
|---|---|---|
| builder | opus | max mandatory |
| tester | opus | max mandatory |
| reviewer | opus | max + adversarial breadth |
| doc-reviewer | sonnet | doc scope wide |
| loop-rounds | 3 | max default |
| review | adversarial | max default |
| codex | on, sol, **--base bd6f1ee** | max mandatory; deviation: fix delta only — main..bd6f1ee already passed a Codex sol gate this session; re-running full diff duplicates a completed gate |

## Workers per phase

| Phase | Worker | Model |
|---|---|---|
| 1 Discover | orchestrator (context already loaded this session) | — |
| 2 Stub | 1× worker-builder (stubbing): new seams only — containment guard fn, `validate_user_tag`, glob module skeleton, `EntryDescription` Deserialize signature, pre-pack struct | opus |
| 3 Verify arch | worker-reviewer (spec-compliance, post-stub) ∥ worker-architect (ADR compliance) | opus / opus |
| 4 Specify | 1× worker-tester (specification): contracts + edge-case list from plan, verbatim | opus |
| 5 Implement | 1× worker-builder (implementation) — single builder; publish.rs is the hot file, no parallel writers | opus |
| 6 Review-Fix | Stage 1 (2 ∥) + Stage 2 adversarial (6 ∥), ≤3 rounds, diff-scoped to bd6f1ee..HEAD | opus (doc: sonnet) |
| 6b Codex gate | codex-adversary code-diff --base bd6f1ee --model sol, one-shot; actionable → one opus fix pass | sol |
| 7 Commit | orchestrator; conventional commits, hook demands real `task verify` first; **no push** | — |

## Estimated cost

Heavier than the review: 4 opus solo phases + up to 3 adversarial rounds
(8 workers each worst case) + Codex sol. Worst case ~20 worker launches.
Loop exits early on convergence — realistic: Stub/Specify/Implement +
1-2 rounds.

## Not Doing

- No push, no PR creation
- Monorepo opt-in flag; sync-fs planning; MCP seam amortization;
  cosmetics deferred (full list in plan artifact)
