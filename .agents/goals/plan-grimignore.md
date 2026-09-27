# Goal: "`.grimignore` — exclude runtime junk from packing and drift hashing"

Source: .agents/plans/plan_grimignore.md · Written: 2026-09-27 by /hex-loop

## Definition of done

- [x] "every WP merged" — evidence: single WP on `hex/plan-grimignore`: 84d91171, 21389f05, e6c292cf, a11f43a4;
  /hex-review Approve (d6b68b8a), `task verify` green.
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

- Entry point: Run the /hex-execute skill on .agents/plans/plan_grimignore.md.
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

## Decisions

- /hex-finalize pre-flight halt 2 ("not the primary checkout") → does not
  apply. Research: this session runs in the human sibling worktree
  `grimoire-duo` and created `hex/plan-grimignore` there; the primary
  checkout `~/dev/grimoire` is on another branch owned by another session,
  so the halt's `Fix:` would hijack that checkout. The halt's stated
  rationale (rewriting a branch this session did not open) does not hold.
  Same decision as the earlier harness-capability-freshness loop (retro
  inbox 20260927T080011Z-f939981c). Proceeded from `grimoire-duo`.

## Rules

None.

## Emphasis

medium/small size

## Context

- Source: `.agents/plans/plan_grimignore.md` (untracked when this file was
  written — commit it alongside this goal file)
- Issue: [grimoire-rs/grimoire#128](https://github.com/grimoire-rs/grimoire/issues/128)
- `.agents/adr/adr_artifact_trust_model.md`, `docs/src/content/docs/stability.md`
