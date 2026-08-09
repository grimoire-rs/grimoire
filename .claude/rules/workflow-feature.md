---
paths:
  - ".agents/plans/**"
  - ".agents/adr/**"
  - ".agents/specs/**"
  - ".agents/research/**"
---

# Feature Development Workflow

Planning through quality gates for feature work. Referenced from [workflow-intent.md](./workflow-intent.md) when work is classified as a feature. Plan artifacts use `plan.template.md` from `.claude/templates/artifacts/`.

## hex Workflow (Primary)

Multi-agent orchestration is the **hex** bundle, installed at user level (not in this repo). Its contracts — tier grammar, worker personas, the Review-Fix Loop, the adversary gate — live in the bundle's own reference files, not here; this rule only states how Grimoire uses it. Project-specific settings (model matrix, always-on review perspectives, research axes, cross-model adversary) live in `.agents/memory/hex.md`, written by `/hex-init`.

### Planning Phase

1. **Plan** — Human describes the feature. Run `/hex-plan` (tier `low | medium | high`, `auto` by default — scales research depth, whether an architect designs, and review breadth). For a standalone architecture decision, run `/hex-architect` instead; it produces an ADR.
2. **Research** — hex spawns researchers per axis. Persist substantial findings as `.agents/research/research_[topic].md`.
3. **Design** — the architect reads subsystem context rules, code and research artifacts, then writes the plan to `.agents/plans/`. A plan must carry testable component contracts and UX scenarios.
4. **Review** — human reviews and approves the plan at hex's meta-plan gate.

### Execution Phase (Contract-First TDD)

Run `/hex-execute` (tier `auto` by default, read from the plan header). Tier scales review breadth, Review-Fix Loop rounds, and the cross-model code-diff gate. File-disjoint work packages run in parallel worktrees under `.agents/worktrees/` and merge onto the feature branch in topological order.

5. **Stub** — type signatures, traits, function shells with `unimplemented!()` / `raise NotImplementedError`. Gate: `cargo check` passes.
6. **Specify** — unit + acceptance tests written from the design record, not from the stubs. Gate: tests compile and fail with `unimplemented`.
7. **Implement** — stub bodies filled until all tests pass. Gate: **subsystem verify** for the changed area (e.g. `task rust:verify`), not full `task verify`.
8. **Review-Fix Loop** — bounded, diff-scoped, tier-capped rounds. Always-on perspectives for this repo (security on credential/publish paths, doc-reviewer on `src/command/**`) are declared in `.agents/memory/hex.md`.
9. **Commit** — changes committed on the feature branch with a Conventional Commit message; deferred findings printed as a summary.
10. **Push** — the human decides when to push (CI cost is real).

Before landing, run `/hex-review` on the branch diff, then `/finalize`.

## Agent Team Workflow (Experimental)

Enable `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1`. Multiple Claude sessions coordinate via shared task lists, same contract-first TDD order: architect plans → builder stubs → reviewer verifies stubs against the design record → tester specifies → builder implements → reviewer checks the diff → iterate on actionable findings (max 3 rounds) → all teammates commit on the feature branch, human decides push.

### Team Sizing

- 3-5 teammates optimal (coordination overhead grows with size)
- ~5-6 tasks per teammate
- Avoid two teammates editing the same file

## Plan Status Tracking

Every plan in `.agents/plans/plan_*.md` carries a `## Status` block at the top (after H1, before the first content section). Block fields: `Plan`, `Active phase`, `Step`, `Last update`. Written by `/hex-plan` (init) and advanced by `/hex-execute` and `/hex-review`; `/commit` bumps `Last update`, `/finalize` refuses while a plan is active and marks it `finalized` on completion, `/next` reads it as its primary signal. Global pointer `.claude/state/current_plan.md` (gitignored) names the active plan; hex additionally tracks the active plan in `.agents/memory/hex.md` › Memory. Schema + mutation table → [`meta-ai-config.md`](./meta-ai-config.md) "Plan Status Protocol".

## Quality Gates

Run `task verify` (fmt check + clippy + build + unit tests + acceptance tests). See `.claude/rules/quality-core.md` for the canonical gate list.
