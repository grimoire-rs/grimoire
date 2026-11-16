# Goal: "Install records as a cache of the disk"

Source: .agents/plans/plan_install_record_cache.md · Written: 2026-11-15 by /hex-loop

## Definition of done

- [x] "every pipeline landed" — evidence: merge commits on
  `hex/plan-install-record-cache`: contract wave `3eea9b21`, P1 `f50eb7ad`,
  P2 `31d644b3`, P3 `5af67db7`, P4 `28f475f4`, P5 `7ac7cbe9`, P6 `3ff7de56`,
  P7 `ea84d273`, P8 `74a454f2`; integration gate
  `env -u OCX_GLOBAL task --force verify` exit 0 on `1b9827aa` (7308 unit
  tests, 3877 acceptance passed, 196 skipped).
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

- Entry point: Run the /hex-execute skill on .agents/plans/plan_install_record_cache.md.
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

- Source: `.agents/plans/plan_install_record_cache.md` (untracked when this
  file was written — commit it alongside this goal file, together with the
  ADR, the dossier and the research files it links).
- Design record: `.agents/adr/adr_output_ownership_without_state.md`.
- Test design: `.agents/research/research_install_record_cache_test_matrix.md`.
- Dossier: `.agents/discussions/stale-outputs-without-state.md`.
- The plan's header names the branch `hex/install-record-cache`. The branch
  the pasted prompt's I9 names is the one this run uses.
- The plan's § Open Questions hold three questions with recommended answers
  that the owner had not answered when this file was written.
- Decision record (§ Issue resolution step 2), 2026-11-15. Question: how
  does the run treat the plan's three open questions, which the owner did
  not answer? Research: the plan's own analysis; each question carries a
  recommendation, its reason and what declining removes. Decision: the run
  executes all three at the recommended answer (ratify the ADR's behaviour
  changes; judge each output on its own when one artifact sits at two
  versions; give `grim lock` and `grim add` the `remove` pre-pass). This is
  the run's decision, not an owner ruling. The paste disclosure told the
  owner before the run started. The PR body lists the three for the owner,
  with what each decline would remove.
- Execute pass, 2026-11-16 (outer cycle 0). Its doubts and decisions are in
  the plan's § Notes. Carried forward, not decided by the owner:
  - `grim update` fetches old pins with no deadline (planner's ruling R-10
    keeps deadlines off shipped commands); a follow-up is owed.
  - Four residual security findings, by location and class only: the
    `grim login` verify request (credential transport to a host only a
    project file marks insecure); environment-carried insecure hosts and
    `GRIM_HOME` (trust of a repository-settable surface); an HTTPS
    challenge naming a plain-HTTP realm (credential transport, fix lives in
    `external/rust-oci-client`); the manifest size cap applied after the
    read (resource bound, same submodule). Not filed as issues.
  - Windows proof is owed to a CI deep run.
  - P8 docs ran before the review; a docs touch-up follows the last code
    fix if the review changes behaviour.
- Review call 1 of 3, 2026-11-16 (tier xhigh, codex `gpt-6.1-sol` ran):
  Request Changes, recorded in `b77acc4b`. The plan's rows RF1 to RF5 carry
  the fixes. Outer cycle 1 of 2 starts with the /hex-execute pass over them.
- Decision record, 2026-11-16. Question: the review rates the loader
  typestate "label-only" (`adopt`, `defer` and `exempt` return the state
  without running a proof), where the ADR's B8 shape forced the proof to
  run; change it? Research: the plan's contract C-019 and ruling R-9 ask
  for exactly this shape, a compile-time choice at every load, and the
  review found no caller that picks the wrong class. Decision: keep C-019
  as contracted; no change in the fix pass. This is the run's decision and
  a known difference from the ADR. The PR body names it for the owner.
- Fix pass, 2026-11-16 (outer cycle 1 of 2 spent): RF1 to RF5 merged, head
  `4a9cda24`; `env -u OCX_GLOBAL task --force verify` exit 0 (7344 unit
  tests, 3906 acceptance passed, 196 skipped), `task test:old-grim` exit 0
  (132 passed), `task docs:check` exit 0. Its decisions are in the plan's
  § Notes. One divergence from the review's fix list: the unproven-files
  line keeps "delete it by hand" and gains no "retry online" advice,
  because the node has left the lock and a second run cannot prove it.
  Residuals left unfixed, low severity: the state lock file still sits
  inside a linked `.grimoire/`; global scope creates `$GRIM_HOME/state`
  before its re-check; one extra hash for a row-4 node; a mid-run save can
  block up to 2 s.
- Review call 2 of 3, 2026-11-16 (convergence check over the fix delta,
  codex `gpt-6.1-sol` ran): Request Changes, recorded in `45b1cff4`; rows
  RF6 to RF8. Convergence against review 1: credential transport,
  compatibility and tests converged; ownership proof, fetching status and
  docs still moving. Outer cycle 2 of 2 starts with the /hex-execute pass
  over RF6 to RF8; it is the last fix pass the cap allows.
- Decision record, 2026-11-16. Question: two review calls each found a
  link-read defect in a different reader, and the decoded-bytes cap was
  closed only on one path; how does the last fix pass avoid a third
  instance? Research: both review reports; the instances share a class
  (a gesture-free path reads a repository-controlled destination before
  containment or without a bound; a decode runs outside the run budget).
  Decision: the fix pass closes the two classes, not only the cited lines:
  it lists every read of a project-scope destination on the gesture-free
  paths (plain `status`, read-only `grim_status`, TUI open) and every
  decode on those paths, and proves each contained, capped and charged.
  Review call 3 then checks that list.
- Owner decision, 2026-11-16 (in session, after review call 2): a third
  fix pass is allowed if needed, beyond the two outer cycles of § Loop
  shape. The owner reads the loop as converging: no functional finding is
  open, the uncapped read is hardening against a hostile repository, and a
  wrong doc sentence is not a blocker. Consequence for the run (the run's
  reading of that decision): from here a review gates only on a functional
  defect (a wrong delete or overwrite, a credential sent where it must not
  go, a compatibility break). Doc accuracy and hardening findings are
  Warns: fixed when cheap, otherwise listed in the PR.
- Fix pass 2, 2026-11-16 (outer cycle 2 of 2 spent): RF6 to RF8 merged,
  code head `7ecbcd9b`; `env -u OCX_GLOBAL task --force verify` exit 0
  (7366 unit tests, 3907 acceptance passed, 196 skipped),
  `task test:old-grim` exit 0 (132 passed), `task docs:check` exit 0. The
  class-wide sweep the run had asked for was narrowed after the owner's
  decision: the plan's § Notes list 18 read sites (4 gaps fixed, 6 left
  open) and 14 decode rows (5 fixed, 3 left open, each bounded per run or
  per call). The open rows are hardening, not functional defects.
- Review call 3 of 3, 2026-11-16 (closing review over the fix pass 2
  delta, codex `gpt-6.1-sol` ran): Approve, recorded in `b517b3c3`. No
  functional defect; every item of review call 2 closed; every area
  converged. Left as Warns: one over-broad sentence on the status page
  about a linked project MCP config, and one missing test for two
  candidates that share a good layer. The doc sentence is corrected before
  the landing; the test gap goes into the PR body. The third fix pass the
  owner allowed is not used.
- Landing, 2026-11-16. The trunk moved 16 commits and conflicted in two
  files (`docs/src/content/docs/upgrading.md`,
  `.claude/rules/subsystem-cli-commands.md`). The trunk is merged into the
  branch in `e88b2db9`, both sides kept; `cargo check --all-targets`,
  `task docs:check`, `task claude:tests` and `task catalog:verify` exit 0.
  The release gate runs on that merged tree.
- Decision record, 2026-11-16. Question: /hex-finalize halts when it is
  "not the primary checkout"; this session runs in the linked worktree
  `duo`, and the repository's first checkout holds another run's branch.
  Does the halt apply? Research: the halt's text and reason target an
  agent worktree, "a branch this session did not open"; this session
  created the branch here, the pasted prompt's I9 names this branch and
  this session, the halt's own fix (switch the branch in the first
  checkout) cannot run while the branch is checked out here, and earlier
  runs in this repository finalized from the same kind of worktree.
  Decision: the halt does not apply; finalize runs here. This is the
  run's decision, disclosed in the finalize gate and to the owner.
