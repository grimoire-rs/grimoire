# Goal: "Recurring harness-capability freshness loop"

Source: .agents/discussions/harness-capability-freshness.md · Written: 2026-09-27 by /hex-loop

## Definition of done

- [x] "/upstream-refresh skill with harness and --domain modes, linked from the watchlist" — evidence: `.claude/skills/upstream-refresh/SKILL.md` (d7ebaa2e, simplified 56912e96); watchlist Re-verify procedure links it.
- [x] "Domain registry covers all six domains with ledger, sources and feed" — evidence: `.claude/skills/upstream-refresh/references/domains.md`, six domains (d7ebaa2e, specs widened 9f483b27).
- [x] "Check ledger .agents/upstream-checks.md records last check, depth, feed cursor" — evidence: `.agents/upstream-checks.md`, one row per harness and domain with Last check, Depth, Feed cursor, Last deep (d7ebaa2e).
- [x] "Depth ladder: same day no-op, 1–2 days feeds only, 3+ days sweep, Tier 1 deep at 30 days or when named" — evidence: `upstream_stale.py ladder` (afef7181): 0 days noop, 1–2 feed, 3+ sweep, Tier 1 prints deep at 30 days; deep runs on a named harness, a default run reports it due (owner directive 2026-09-27, 56912e96).
- [x] "Every changed claim carries verified date, source URL, vendor version if known" — evidence: per-pass `.agents/research/research_upstream_<target>_20260927.md` `## Claims`; dated watchlist rows in 9346104d, efc6c7ec, ddb8874b, 528dcb71, faf4ecb6.
- [x] "Normal sweep runs stale-first, one sonnet researcher per domain or harness" — evidence: Tier 2 sweep faf4ecb6 (one sonnet researcher per harness); default run fans out all targets at once (56912e96).
- [x] "Deep pass: full surface inventory plus live CLI check" — evidence: Tier 1 passes 9346104d, efc6c7ec, ddb8874b, 528dcb71 with `references/deep-pass.md`; live CLI in throwaway HOME (codex, copilot live; skips recorded).
- [x] "Changed claims cite URL and quote; opus verifies code-driving claims" — evidence: `## Claims` URL + quote and `## Verifier sign-off` (opus) in each pass artifact; code landed 2ecdfc03, 372e74d9, c76d25f2, 845ea76f.
- [x] "Findings routed by risk: direct fix, additive support with tests, else issue" — evidence: direct fixes 4200c434, b71b6bca; additive support with tests 2ecdfc03, 372e74d9, c76d25f2, 845ea76f; issues #139–#156.
- [x] "task upstream:stale lists stale rows and expired research, warn-only" — evidence: `taskfiles/upstream.taskfile.yml` (afef7181), outside `task verify`; reports 0 stale, 0 expired, 3 unchecked (deferred forge, landscape, research).
- [x] "Skill refined after each run from recorded friction" — evidence: refinement commits 37aa6612, 0a5d8296, e5e01bf4, bb0bf8f7, f9effcc1, 9f483b27, 56912e96.
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

## Loop shape

- Entry point: Run the /hex-plan skill on "Recurring harness-capability freshness loop, per .agents/discussions/harness-capability-freshness.md".
- Refinement rounds: 2 — counts outer cycles: each review ⇄ execute
  pass is one, and so is every failed repair or retry cycle — a local
  verify failure, an execute or finalize retry, a post-finalize CI fix ⇄
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

From the source's Decisions and Verification: build first (the skill, the
staleness task, the check ledger), then deep passes on the Tier 1
harnesses `claude`, `codex`, `opencode`, `copilot` one at a time, then the
Tier 2 vendor sweep, then the catalog, spec, forge, product-landscape and
expired-research domains. The skill is refined after every pass; the work
is done when every domain has been swept once, `task upstream:stale`
reports nothing past the threshold, and the last pass needed no skill
change. Principle 9 (additive-only) binds every renderer change.

## Context

- Source: `.agents/discussions/harness-capability-freshness.md`
- Research: `.agents/research/research_upstream_claim_surfaces.md`,
  `.agents/research/research_capability_matrix_freshness.md`,
  `.agents/research/research_multi_harness_tool_freshness.md`,
  `.agents/research/research_harness_change_feeds.md`,
  `.agents/research/research_repo_reverification_history.md`
- `.claude/rules/vendor-capability-watchlist.md`,
  `.agents/adr/adr_vendor_support_tiers.md`
