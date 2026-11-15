# Goal: "Dependency tags as version claims"

Source: .agents/plans/plan_dependency_version_claims.md · Written: 2026-11-12 by /hex-loop

## Definition of done

- [x] every pipeline landed — evidence: all nine rows of the plan's
  Parallelization table read `merged`; landing commits b38eedbd, 894e476e,
  e4fce898, dc8b2cad, 868e0e4f, 16bf3c39, 3c33ded3, 26fb7513, 57725385 and
  81a03292; the review-fix pass landed as 7c67bd5e..e4027d04 after a green
  `task --force verify` (3393 passed, 193 skipped, 0 failed).
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
- Forbidden or narrowed acts:
  - Never push and never open a PR; stop before /hex-finalize.

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

- Entry point: Run the /hex-execute skill on .agents/plans/plan_dependency_version_claims.md.
- Refinement rounds: 2 — counts outer cycles: each review ⇄ execute
  pass is one, and so is every failed repair or retry cycle — a red
  gate's fix pass, an execute or finalize retry, a post-finalize CI fix ⇄
  re-finalize pass. Inner review-fix rounds do not count, and their limit
  is untouched.
  Past `2`, the DONE block reports every remaining criterion `not met`
  and the run stops.
- Outer cycles used: 1 of 2 (2026-11-13: the closing /hex-review returned
  Request Changes, 3 Block and 25 Warn; this review-fix pass spends cycle 1). Update this line, with the date and the pass
  that spent the cycle, each time one is spent.
- Ticks: /hex-loop commits nothing. The session creates or switches to
  the branch the pasted prompt's I9 names and commits this file first on
  it. A box is ticked only between hex-mode runs, never during one, and
  each tick is committed at once; every tick lands before the final
  /hex-finalize, none after it. The `(DONE block only)` criteria are
  evidenced only in the closing DONE block, never ticked.

## Rules

- Run rules:
  - After every pipeline has landed and before the closing review, spawn 5
    sonnet subagents that act as users and try to break dependency version
    resolution with the built binary, each in its own isolated environment
    (own GRIM_HOME, own workspace, a local registry).
  - Every bug they confirm gets a failing regression test first, then the fix.
  - Then run the review and fix loop.

## Emphasis

None.

## Context

- Plan (the source): `.agents/plans/plan_dependency_version_claims.md`
- Decision record: `.agents/adr/adr_dependency_version_claims.md`
- Run records (2026-11-12):
  - The goal started while `/hex-execute` was mid-run: 4 of 9 pipelines
    landed, the graph pipeline in flight in its lane. The run continues
    from that state; nothing is restarted.
  - Branch: the local branch `feat/dependency-version-claims` was renamed to
    `hex/plan-dependency-version-claims`. It was never pushed.
  - Commits: a repository hook refuses every commit without a `task verify`
    stamp under 300 s old, and `--no-verify` does not pass it. Each landing
    therefore runs `env -u OCX_GLOBAL task --force verify` and commits inside
    that window. This file is committed first in the next window, ahead of
    the graph pipeline's commits.
- Doubts (2026-11-13), per § Issue resolution:
  - Question: the graph pipeline's review found that a plain `grim lock`
    moves a held tagless, unannotated pin when only the registry moved a
    tag, and that the exit code can flip from 0 to 78 the same way. The
    decision record contradicts itself on both.
    Research: delegated to a sub-orchestrator; its record is
    `.agents/.tmp/version-claims/pd-doubts.md`, and the outcomes land in
    the decision record's changelog.
    Decision (the main session's, not the owner's): the standing rule
    wins, a plain relock never moves a held pin; such a pin is judged by
    the tag its recorded edge names. The remaining review readings are
    decided by the same precedence: owner decisions, then the decision
    record's text, then compatibility with released locks.
- User-level bug hunt (2026-11-13): five sonnet agents, each with its own
  `GRIM_HOME`, workspace and local registry, reported 16 items. Triage and
  rulings: `.agents/.tmp/version-claims/bughunt/triage.md`; the outcomes are
  in the decision record's changelog. Every confirmed in-scope bug got a
  failing regression test first, then the fix.
  - Deferred, out of scope:
    [#358](https://github.com/grimoire-rs/grimoire/issues/358) (offline
    `grim add` blames the kind).
  - Deferred, residual:
    [#359](https://github.com/grimoire-rs/grimoire/issues/359) (concurrent
    `grim release` runs can leave a float on a lower version).
- Closing review and fix pass (2026-11-13): the one closing `/hex-review`
  returned Request Changes (3 Block, 25 Warn, 32 Suggest). The review-fix
  pass fixed every Block and Warn, each with a failing test first, in two
  lanes and a seam pass; the full gate is green on the result. Rulings on
  the review's judgment list are the main session's, not the owner's:
  `.agents/.tmp/version-claims/final-review-rulings.md`, and the decision
  record's changelog rows dated 2026-11-13.
  - The widened pin harness found one more defect, new on this branch
    (`grim remove` dropped a pin a remaining version claim still held); it
    is fixed with regression tests.
  - Deferred, refactor:
    [#360](https://github.com/grimoire-rs/grimoire/issues/360) (split the
    `Graph` struct).
  - Security findings, by reference only: the release float rule's trust in
    publisher-set version metadata (decision record, Residuals); the tag
    listing limits in `src/oci/access/registry_client.rs` (fixed).
  - No review call remains (3 of 3 used). The run stops before
    `/hex-finalize`, as § Autonomy narrows it.
