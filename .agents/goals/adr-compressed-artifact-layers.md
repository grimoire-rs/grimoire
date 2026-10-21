# Goal: Compressed artifact layers (reader-first, opt-in gzip)

Source: .agents/adr/adr_compressed_artifact_layers.md · Written: 2026-10-21 by /hex-loop

## Definition of done

- [x] D1 readers first: gzip tar layers decode on the tar read path; +zstd exits 65 — evidence: the commit, PR comment or
  artifact that satisfies it.
- [x] D2 opt-in writer: release/publish --compression <none|gzip>, default none — evidence: the commit, PR comment or
  artifact that satisfies it.
- [x] D3 scope by kind: tar kinds only; bundle/mcp ignore the setting — evidence: the commit, PR comment or
  artifact that satisfies it.
- [x] D4 compressed layers carry com.grimoire.uncompressed.digest, checked on read — evidence: the commit, PR comment or
  artifact that satisfies it.
- [x] D5 release guard: content-key compare adopts equal tar-kind re-releases, else TagExists — evidence: the commit, PR comment or
  artifact that satisfies it.
- [x] D6 decoded bytes capped at min(caller cap, max(64 MiB, 100x stored)) — evidence: the commit, PR comment or
  artifact that satisfies it.
- [x] D7 pull digests, blob store keys, lock hash and build layer_digest unchanged — evidence: the commit, PR comment or
  artifact that satisfies it.
- [x] D8 describe exposes compression: none, gzip or null — evidence: the commit, PR comment or
  artifact that satisfies it.
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

- Entry point: Run the /hex-plan skill on "Compressed artifact layers (reader-first, opt-in gzip), per .agents/adr/adr_compressed_artifact_layers.md".
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

- Run rules:
  - None.

## Emphasis

None.

## Context

- ADR: .agents/adr/adr_compressed_artifact_layers.md
- Discussion: .agents/discussions/compressed-artifact-layers.md
- Existing implementation: https://github.com/grimoire-rs/grimoire/pull/209 (branch feat/compressed-layers)
- 2026-10-21 — D1–D8 ticked after /hex-execute of `.agents/plans/plan_compressed_artifact_layers.md` (gate green: 4195 unit, 1773 acceptance; one review, converged). Per-decision test evidence is in the plan's handoff and the PR body. grim 0.14.3 `install` of a gzip artifact exits 65 with no client output written (recorded in the ADR).
