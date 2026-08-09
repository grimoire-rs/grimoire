# Meta-Plan: /swarm-execute plan_vendor_wave_expansion.md

## Classification

- **Tier:** max (plan header, verbatim; confident)
- **Overlays:** builder=opus (mandatory), tester=opus (mandatory),
  reviewer=opus (adversarial breadth trigger), doc-reviewer=sonnet,
  loop-rounds=3, review=adversarial, codex=on→**will skip**
  (`codex-adversary` is user-invocable-only; surfaced prominently, user
  can run `/codex-adversary code-diff` manually post-commit).

## Cost reality (the reason this gate matters)

Plan = 7 tranches (G groundwork; V1 Cursor … V6 Amp). Stock max ceremony
per tranche: opus stub + reviewer/architect arch-verify + opus tester +
opus builder + 3-round adversarial panel (~6 perspectives) ≈ 10–12 worker
launches/tranche → ~75+ launches, heavily opus, for the full plan in one
run.

## Recommended scoping (deviation from "run everything")

**This run: tranche G + tranche V1 (Cursor) only, full max ceremony.**
- G validates the load-bearing shared machinery (tri-state, refcount
  guard on a delete path, namespace derive, matrix parity test).
- V1 is the richest vendor (4/4 kinds) — becomes the reviewed template
  the remaining five vendors copy.
- V2–V6 then run as follow-up `/swarm-execute` continuations where the
  per-vendor marginal ceremony can drop (template proven; suggest
  review=full instead of adversarial for the repetition tranches —
  decided at their own gate).

Alternatives: (b) full 7-tranche run now (~75 launches, very long,
single-session risk); (c) G only (defers all user-visible value).

## Workers this run (G + V1)

| Phase | Worker | Model |
|---|---|---|
| Stub (per tranche) | worker-builder (stubbing) | opus |
| Verify arch | worker-reviewer (spec-compliance) + worker-architect | sonnet / opus |
| Specify | worker-tester | opus |
| Implement | worker-builder (implementation) | opus |
| Review loop ≤3 rounds | Stage 1: 2 reviewers; Stage 2 adversarial: quality+security+performance+cli-ux reviewers, doc-reviewer, architect, researcher | opus reviewers (adversarial trigger), doc sonnet |
| Cross-model | codex-adversary code-diff sol | SKIPPED (user-invocable only) |

Max 8 concurrent respected; phases sequential per tranche.

## Git

Worktree `../grimoire-wt-vendor-wave` on branch `feat/vendor-wave-expansion`
(created at start; all git via `git -C`, per worktree-commit discipline).
Conventional commits per tranche (`feat(install): …`). **No push.**

## Not Doing

- No push, no PR, no release.
- No V2–V6 this run (follow-up continuations).
- No ADR status flip until you approve execution (flip = part of G commit).
