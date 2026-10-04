# Goal: "Revive PR #98 (hooks artifact kind) for release"

Source: .agents/discussions/hooks-pr-98-revival.md · Written: 2026-09-28 by /hex-loop

## Definition of done

- [ ] "PR #98 branch squashed and rebased once onto main; /finalize re-splits it" — evidence: the commit, PR comment or artifact that satisfies it.
- [x] "Hook install ported onto main's extracted install seams and vendor rewrites" — evidence: c650e93e, 083f6ef4, 9455d5ca; review fixes bb0379c9, 6ff7ef56.
- [x] "Qoder hooks and Copilot mutator (modifiedArgs) wired" — evidence: Qoder c6c7c4d6, 0e181f83 (D-19, global-only); Copilot mutator ships as `updatedInput` — live probe applied it so the `modifiedArgs` fallback was unneeded (D-5, research_hooks_qoder_copilot_schemas.md §6).
- [x] "Hook ADR facts corrected, amendments and watchlist hook rows recorded" — evidence: 4e0f87aa.
- [x] "grim export plugin declines hook members with a warning and README listing" — evidence: de716596, 82763f36.
- [x] "Hooks docs on Starlight: use-case page plus reference entries" — evidence: cefa3437, 31e727e1, cdda4403 (docs/src/content/docs/hooks.md).
- [x] "Rules added since the base obeyed: Principle 9, config keys, catalog drift, docs-quality" — evidence: P9 d33701dd, 9616f426, a815136b, 5be3b22a (p9:diff 0 diffs); config keys config_keys.rs; catalog 559167c2, 0e48a5a9 (catalog:verify green); docs-quality 31e727e1 (docs:check green).
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

- Entry point: Run the /hex-plan skill on "Revive PR #98 (hooks artifact kind) for release, per .agents/discussions/hooks-pr-98-revival.md".
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

- Run rules:
  - None.

## Emphasis

From the source: Principle 9 — hook-free projects stay byte-identical and flag-off installs register no dispatcher; hooks ship experimental, `[options.experimental] hooks` off by default; Windows acceptance reproduced locally against `origin/main` before blaming the branch; the open questions (Qoder scope, Copilot cloud agent) carry recommendations in the source.

## Context

- Source: `.agents/discussions/hooks-pr-98-revival.md` (ratified 2026-09-28 → loop).
- PR: https://github.com/grimoire-rs/grimoire/pull/98 (branch `hex/hooks-artifact-kind`).
- Research: `.agents/research/research_hooks_pr98_drift.md`, `.agents/research/research_hooks_upstream_2026-09.md`.
- On the PR branch: `.agents/adr/adr_hooks_support.md`, `.agents/adr/adr_hook_workspace_consent.md`, `.agents/plans/plan_hooks_artifact_kind.md`.
- On main: `.agents/adr/adr_harness_plugin_export.md`, `.agents/adr/adr_render_layout_stability.md`, `.agents/plans/plan_docs_site_redesign.md`.
