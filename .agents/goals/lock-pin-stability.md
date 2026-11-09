# Goal: Lock pin stability

Source: .agents/discussions/lock-pin-stability.md · Written: 2026-11-03 by /hex-loop

## Definition of done

- [x] R1 only grim update moves pins: lock, add, TUI, remove-then-lock keep them, even on moved tags — evidence: the commit, PR comment or
  artifact that satisfies it; covers the source's Requirements › Lock semantics (first two bullets) and its Verification regression list.
- [x] R2 every pin records its resolving reference; per-node carry; old locks migrate; 0.15.1 reads them — evidence: the commit, PR comment or
  artifact that satisfies it; covers the rest of Requirements › Lock semantics, the shared-reference probe, and the 0.15.1 interop check.
- [x] R3 Refresh set and plan_lock dispositions replace relock_* and partial paths — evidence: the commit, PR comment or
  artifact that satisfies it; covers Requirements › Command architecture (lock half, reports, hygiene).
- [x] R4 #269: one install scope per pass, scoped force/prune, restored action, converge extracted — evidence: the commit, PR comment or
  artifact that satisfies it; covers Requirements › Command architecture (install half) and issue #269 cases A and B.
- [x] R5 marketplace.lock follows the same pin rule — evidence: the commit, PR comment or
  artifact that satisfies it; covers Requirements › Marketplace.
- [x] R6 edge-case wave: fixes test-first, kept tests justified — evidence: the commit, PR comment or
  artifact that satisfies it; covers Requirements › Edge-case hunt and acceptance tests (the wave and the keep-rule).
- [x] R7 acceptance suite sharded in CI, JUnit merge job — evidence: the commit, PR comment or
  artifact that satisfies it; covers the sharding bullet and its Verification line.
- [x] R8 docs, ADR Decision 3, test flip, catalog drift updated — evidence: the commit, PR comment or
  artifact that satisfies it; covers Requirements › Contracts that change with the fix.
- [x] R9 vscode PR adopts grim's changed JSON — evidence: the commit, PR comment or
  artifact that satisfies it; covers Requirements › Adoption in the other repositories.
- [x] R10 indexer PR shows declared dependencies — evidence: the commit, PR comment or
  artifact that satisfies it; covers Requirements › Adoption in the other repositories.
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
- Granted acts (authority: the pasted prompt): beyond I9's defaults,
  push, force-push with lease, and one ready PR in
  grimoire-rs/grimoire-vscode and grimoire-rs/indexer, for R9 and R10.
  Merge stays the owner's.
- Forbidden or narrowed acts: in grimoire-rs/grimoire-vscode and
  grimoire-rs/indexer, pushes and the PR use branch hex/lock-pin-stability
  only.

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

Owner decisions for the resumed run (2026-11-05), answering review 3's
open questions:

- The empty `declaration_hash` is dropped: an edit that leaves a lock
  unproven writes a well-formed hash no declaration produces.
- PD-29: `grim add` keeps a pin its reference already answers.
- PD-32: a marketplace part keeps its bundles' pins in the existing
  `bundle_tag` string; if 0.15.1 cannot read that, R5 is ratified as
  partial and PD-32 is reworded.
- Scope: every Block, High and Warn of review 3, plus the follow-ups
  review 3 reclassified as defects.

Owner decisions for the third run (2026-11-07), after review 6 requested
changes with the resumed run's limits spent. These relax the criteria:

- Limits: two more outer cycles and at most two /hex-review calls
  (reviews 7 and 8). The start is the plan's § RF6.
- Scope: every Block and High of review 6, and of review 7 if it runs.
  A Warn is fixed in the same pass only if it meets one of these:
  - it is a defect a user can hit in R1–R8's named behaviour;
  - it is a docs accuracy row;
  - it sits in code the pass already changes.

  Every other Warn, and every Suggest, becomes a follow-up issue and
  does not hold a criterion unmet. Security items stay by reference and
  are never filed.
- Recorded residuals: an owner-flagged trade does not hold its criterion
  unmet when two things are true: the plan records its reach and cost,
  and the DONE block lists it for the owner. This covers RD4-11,
  RD5-23, RD5-38, RD5-61, RD5-28 and the proposed narrowings of the
  consent and trust ADRs (RD4-3, A3, A4). If a review shows such a record
  to be false, it is a defect again.
- Lock shapes: R1 and R2 are judged on these:
  - locks grim 0.15.x writes;
  - edits a user makes to `grimoire.toml`, including deleting a line;
  - the documented upgrade paths.

  Locks cut by hand, and locks written by grim 0.3 or earlier, are
  recorded residuals.
- R6: a surviving harness mutant does not hold R6 unmet when its reason
  is recorded: it is equivalent, or a named unit test kills it.
- R7: met by the committed workflow; its first CI run on the PR is the
  evidence, under "CI green per job".
- Principle 9 stays the hard limit. A lock 0.15.1 refuses (review 6's B2)
  is fixed, never recorded. A new deviation row needs the owner.

Owner decisions of 2026-11-08, after review 8 requested changes with the
third run's limits spent. Each was answered in the session; the answers
quoted are the owner's:

- Fix now: the missed member conflict check. The owner said "such miss
  conflicked check should be fixed" (landed as FX8).
- Follow-ups: fix issues #326 to #332 in this PR (landed as FU8). Docs:
  "the docs fixes should be solved after implementation, such that
  everything is consistent".
- D-1 (residual 1): "Accept as exception". On a lock 0.15.1 wrote, an
  edit may drop a row only an edge-less dependent needs, and the next
  online `grim lock` may move its pin, until one online write with this
  grim records the edges. It does not hold R1 unmet.
- `team:held`: "Refuse with 78". A part pick that would leave a dependent
  beside a newer version of a row it requires refuses with 78. This
  grants the deviation row for that 0 → 78.
- `grim add` against `grim lock`: "Fix: add keeps pin". Where a moved tag's
  manifest fails its walk (a gone dependency, 79) or cycles back at
  another digest (65), `grim add` keeps the pin and exits 0, as
  `grim lock` does.
- Marketplace part picks: "Redesign now". Part picks decide bundle state
  inside one post-graph step instead of the stamp-keyed prune and
  reconstruction, which closes review 8's root B′ (H2, MK-F1, the part
  Warns).
- Deviation row 4: "Allow 78→0". A part pick beside an unselected
  sibling bundle that the part records keeps the sibling at its recorded
  digest and exits 0 where 0.15.1 and c5e3824 refuse with 78 on its moved
  tag. This covers the 57 measured cells and the redesign's nested case.
  A whole-plugin update still refuses.
- Nested bundle under a picked bundle: "Allow 78→0". A pick of a bundle
  keeps a bundle nested inside it at the digest the part records and exits
  0 where 0.15.1 (65) and c5e3824 (78) follow the nested bundle's moved tag
  and refuse on the conflict (6 measured cells). A pick naming the nested
  bundle, or a whole-plugin update, still moves it.
- Member of a picked bundle: "Allow 78→0". A pick of a bundle keeps its
  members at their pins and exits 0 where 0.15.1 and c5e3824 follow a
  member's moved tag and refuse with 78 on the conflict it brings (2
  measured cells; the part stays byte-identical). A pick naming the member
  still moves it.
- A held requirement as a known name: "Keep 0, like main". On a part
  0.15.1 wrote, a pick of a row its bundle no longer lists but another
  member still requires exits 0, as c5e3824 does, where 0.15.1 exits 79
  (1 measured cell).
- Limits for this fourth stretch (coordinator's, within the owner's
  answers): one redesign pass that also takes every open review 8 Warn,
  then one /hex-review (review 9). At most 2 inner fix rounds per pass.
- After review 9 requested changes (2026-11-08, late): "Fix pass + small
  review". One short pass (RF9) takes review 9's Block B1 test-first, its
  three code Warns, every record and page edit it lists, and the owed CI
  floor commit; a Codex closing read ends it. Then review 10, scoped to
  RF9's delta, and /hex-finalize if it approves.
- After review 10 found no Block but did not approve (2026-11-09): "One
  last fix, then ship". One pass (RF10) makes the held side of row 25's
  check transitive (a required bundle expands through everything it
  nests), finds a gone tag's previous row by the held edge, and applies
  review 10's text edits. A Codex closing read ends it, with no review 11:
  anything that read still finds is recorded as a row 25 residual. Then
  /hex-finalize and the three PRs.

## Loop shape

- Entry point: Run the /hex-execute skill on .agents/plans/plan_lock_pin_stability.md.
- Refinement rounds: 2 — for the third run, from 2026-11-07. Earlier
  runs spent theirs: the first run 2 (2026-11-05), the resumed run 4
  (2026-11-05 to 2026-11-07, ending at review 6). It counts outer cycles:
  each execute ⇄ review pass is one, and so is every failed repair or
  retry cycle. That means a red gate's fix pass, an execute or finalize
  retry, and a post-finalize CI fix ⇄ re-finalize pass. Inner
  review-fix rounds do not count, and their limit is untouched.
  Past `2`, the DONE block reports every remaining criterion `not met`
  and the run stops.
- Inner loop: at most 2 inner fix rounds per hex-mode pass; Codex is the
  cross-model adversary. Each fix pass ends with a Codex read of its last
  fix round's delta. That read is not a fix round. The pass report lists
  any Block or High it finds as open.
- Ticks: /hex-loop commits nothing. The session creates or switches to
  the branch the pasted prompt's I9 names and commits this file first on
  it. A box is ticked only between hex-mode runs, never during one, and
  each tick is committed at once; every tick lands before the final
  /hex-finalize, none after it. The `(DONE block only)` criteria are
  evidenced only in the closing DONE block, never ticked.

## Rules

- Run rules:
  - Every sub-orchestrator brief states the inner fix-round cap: at most
    2 inner fix rounds per hex-mode pass, then stop and report what is
    left.
  - Codex on Sol 6.1 is the cross-model adversary in every /hex-review
    and every inner adversarial read. A native seat substitutes only when
    Codex is unavailable, and the report says so.
  - The vscode and indexer PRs open only after the grim PR is ready. Their
    tests pin a grim built from this branch, and each PR body states that
    grim merges first.
  - At most 2 /hex-review calls in this run, one after each fix pass.
  - The DONE block opens with a plain status before the criterion lines:
    - what works;
    - what is still broken, in user terms;
    - how far from done;
    - the decision needed.

    Security items appear only by location and class, in one short list,
    and only those this branch opened or changed.

## Emphasis

From the source's Decisions and Intent: fix defects first and test-first; Principle 9 is the hard limit (no new lock field, `lock_version` stays 1, report action values only gain new entries, `stability.md` forward compatibility holds); the destructive guards stay as they are and no up-front install planner is built; acceptance tests are added only when they found something or cover a case nothing else does; the source's Out of scope paragraph names what this goal is not, release work included.

## Context

- Source, self-contained: `.agents/discussions/lock-pin-stability.md` (Requirements, Decisions, Open questions, Verification).
- Research: `.agents/research/research_lock_pin_stability_{codebase,prior_art,reconcile,community,archaeology}.md`.
- Issue: https://github.com/grimoire-rs/grimoire/issues/269
- ADRs: `.agents/adr/adr_artifact_dependencies.md`, `.agents/adr/adr_effective_set_mutations.md`.
- Sibling repositories named by R9 and R10: `~/dev/grimoire-rs/vscode`, `~/dev/grimoire-rs/indexer`. OCX prior art: `~/dev/ocx-sh/ocx` (commit `7378fe0b`, `.github/workflows/acceptance.yml`).
- This worktree needs `git submodule update --init` before cargo can build.
