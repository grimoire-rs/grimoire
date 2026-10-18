# Goal: "Index-hosted marketplace — render, gate and deploy"

Source: .agents/plans/plan_marketplace_render_deploy.md · Written: 2026-10-18 by /hex-loop

## Definition of done

- [x] every pipeline landed — evidence: the commit, PR comment or
  artifact that satisfies it. grimoire bfea70ea, b19cc966; indexer
  4e485df, 50dbdbe, 7c45d69 (local); 7/7 pipelines merged per the
  plan's Execution log.
- [ ] PR merge-ready (DONE block only) — evidence: the PR URL, and the
  PR is not a draft.
- [ ] CI green per job (DONE block only) — evidence: every check run on
  the PR head SHA with its conclusion; every skipped job states its
  skip reason (its `if:` or path filter).
  A skip with no reason is not green.
- [ ] Deep verify passed (DONE block only) — evidence: the full documented verification result.

## Autonomy

- Prompting: never — no question waits for a human; a doubt runs
  § Issue resolution.
- Granted acts (authority: the pasted prompt): none
- Forbidden or narrowed acts: None.

## Issue resolution

Every doubt — an ambiguous requirement, a design question, a failure
with an unclear cause — runs this protocol:

1. Delegate the research to a sub-orchestrator.
2. Record question → research → decision in this file or in the PR.
3. Defer to a GitHub issue only in hard cases — the research ends with no
   decision, or the decision needs an act outside the grants; the issue
   link is the recorded decision.
4. Every doubt not deferred ends in an action.
5. A pre-existing failure that blocks done is in scope and runs this
   protocol.
6. Findings outside this goal's scope become follow-up issues.
7. Secrets and credential-bearing logs are never written to any committed
   file, commit message, PR text, PR comment or review comment, or issue.
   Security findings are never filed as issues:
   this file records them by reference only — location and class, no
   secret value or exploit detail — and only the DONE block reports them
   in full.

Recorded decisions:

- Branch base. Question: I9 says create the branch from the trunk, but the
  plan's grimoire base is 34f06a89 (PR #177, unmerged). Decision: branch
  from 34f06a89 and stack the PR on `hex/adr-harness-marketplace-export`;
  a trunk base would drop the export this plan extends.
- Indexer repo. No grant covers it: indexer work is committed locally on
  its existing branch `feat/marketplace-enrich-landing` (worktree
  `.agents/worktrees/indexer-mkt`), never pushed; its PR and CI criteria
  read not met.

## Loop shape

- Entry point: Run the /hex-execute skill on .agents/plans/plan_marketplace_render_deploy.md.
- Refinement rounds: 2 — counts outer cycles: each review ⇄ execute
  pass is one, and so is every failed repair or retry cycle — a red
  gate's fix pass, an execute or finalize retry, a post-finalize CI fix ⇄
  re-finalize pass. Inner review-fix rounds do not count, and their limit
  is untouched.
  Past `2`, the DONE block reports every remaining criterion `not met`
  and the run stops.
- Ticks: /hex-loop commits nothing. The session creates or switches to
  the branch the pasted prompt's I9 names and commits this file first on
  it. A box is ticked only between hex-mode runs, never during one, and
  each tick is committed at once; every tick lands before the final
  /hex-finalize, none after it. The `(DONE block only)` criteria are
  evidenced only in the closing DONE block, never ticked.

## Rules

None.

## Emphasis

None.

## Context

- Source plan: .agents/plans/plan_marketplace_render_deploy.md
- ADR: .agents/adr/adr_harness_marketplace_export.md
- Research: .agents/research/research_url_marketplace_trust.md, .agents/research/research_marketplace_hosting_prior_art.md
