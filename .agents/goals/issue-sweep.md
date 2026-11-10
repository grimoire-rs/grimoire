# Goal: Issue sweep — 16 open issues on one branch

Source: .claude/state/issue-sweep-decisions.md · Written: 2026-11-10 by /hex-loop

## Definition of done

- [ ] #342 Windows acceptance green — evidence: the commit, PR comment or
  artifact that satisfies it; the Windows platform check of `verify-deep.yml` passes on the PR head.
- [x] #341 held-back request warned — evidence: the commit, PR comment or
  artifact that satisfies it.
- [x] #338 lock cleanups — evidence: the commit, PR comment or
  artifact that satisfies it; all four items.
- [x] #336 part-pick shapes judged — evidence: the commit, PR comment or
  artifact that satisfies it; all five items, each with its measured cells.
- [x] #335 global clients join detection — evidence: the commit, PR comment or
  artifact that satisfies it.
- [x] #333 package README companion — evidence: the commit, PR comment or
  artifact that satisfies it; items 1 to 3.
- [x] #317 --log-level applied — evidence: the commit, PR comment or
  artifact that satisfies it.
- [x] #316 bad TOML is an error — evidence: the commit, PR comment or
  artifact that satisfies it.
- [x] #315 .jsonc only where read — evidence: the commit, PR comment or
  artifact that satisfies it.
- [x] #312 global remedy tested — evidence: the commit, PR comment or
  artifact that satisfies it.
- [x] #311 decline warned once — evidence: the commit, PR comment or
  artifact that satisfies it.
- [x] #306 BOM settings read — evidence: the commit, PR comment or
  artifact that satisfies it; or the recorded upstream answer that makes it a note.
- [x] #305 comments survive removal — evidence: the commit, PR comment or
  artifact that satisfies it.
- [x] #301 fetch miss text — evidence: the commit, PR comment or
  artifact that satisfies it.
- [x] #300 unreadable config kept — evidence: the commit, PR comment or
  artifact that satisfies it.
- [x] #299 reap results kept — evidence: the commit, PR comment or
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
- Granted acts (authority: the pasted prompt): one concise PR each in any
  other grimoire-rs repo (grimoire-vscode, indexer) only where this work
  needs it, with push and force-push with lease there on branch
  hex/issue-sweep only; dispatch verify-deep.yml on this branch, at most 3
  runs, to check Windows for #342
- Forbidden or narrowed acts:
  - No push to `main`, no merge, no release, no tag, in any repo.
  - No work on #297 or #257. Both stay open.
  - Do not touch `~/.herdr/worktrees/grimoire/duo/` or the checkouts under
    `~/dev/grimoire-rs/`, except a worktree this run creates there for a
    granted PR, which it removes when that PR is open.
  - Never read a subagent's transcript file.
  - `GRIM_ALLOW_HOOKS` is never re-added. No `Co-Authored-By` trailer.
  - Never hand-write the commit-verified marker, and never edit
    `.claude/hooks/**`.

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

- Entry point: Run the /hex-plan skill on "Issue sweep: 16 issues with settled decisions, per .agents/goals/issue-sweep.md".
- Refinement rounds: 2 — counts outer cycles: each review ⇄ execute
  pass is one, and so is every failed repair or retry cycle — a red
  gate's fix pass, an execute or finalize retry, a post-finalize CI fix ⇄
  re-finalize pass. Inner review-fix rounds do not count, and their limit
  is untouched.
  Past `2`, the DONE block reports every remaining criterion `not met`
  and the run stops.
- Outer cycles used: 1 of 2 (2026-11-10: post-finalize CI fix pass for the macOS unit test, the acceptance skip ceiling and the #341 tag-and-digest remedy). Update this line, with the date and the pass
  that spent the cycle, each time one is spent. The goal check reads it.
- Inner loop: at most 2 inner fix rounds per hex-mode pass. Each fix pass
  ends with one cross-model read of its last round's delta. That read is
  not a fix round, and what it still finds is listed as open.
- Ticks: /hex-loop commits nothing. The session creates or switches to
  the branch the pasted prompt's I9 names and commits this file first on
  it. A box is ticked only between hex-mode runs, never during one, and
  each tick is committed at once; every tick lands before the final
  /hex-finalize, none after it. The `(DONE block only)` criteria are
  evidenced only in the closing DONE block, never ticked.

## Rules

- Run rules:
  - **Shape of the result.** One branch with linear history and one PR. One
    commit per issue, Conventional Commits, signed off (`-s`), with
    `Fixes #N` in the body. #342 and #336 may take more than one commit.
  - **Bugfix discipline.** Every `fix:` follows `.claude/rules/workflow-bugfix.md`:
    reproduce, root cause, a test that fails before the fix, then the fix.
  - **Freeze (Principle 9).** No change to exit codes, JSON output, schemas
    or layouts beyond the owner decisions below. New JSON fields and schema
    keys are optional and additive. A deviation not listed here is a doubt.
  - **Owner decisions (2026-11-10), binding:**
    - #335: when neither `--client` nor the project's `options.clients` is
      set, clients are the detected ones plus the global `options.clients`.
      The flag and a project list stay authoritative. `status` and `install`
      say where each client came from.
    - #333: by default each entry's own `README.md` at its fixed place
      (`skills/<n>/`, `hooks/<n>/`, `agents/<n>/`, `rules/<n>/`) is its
      companion README, and an entry without one keeps the top-level
      README. `[description] entry_readme = "<name>"` picks another file
      name and `false` turns it off. A per-entry table still wins. The owner
      accepts that repositories with package READMEs publish a different
      companion on their next run; note it in the upgrading guide. Items 1
      and 2 ship as the issue states them.
    - #341: no ADR and no pin-moving code. The declared pin holds. `add`,
      `update` and `lock` print one warning that names `grim update -- x`,
      and `grim status --check` lists held-back requests. Exits and existing
      JSON are unchanged. Close the policy question on the issue with this
      ruling.
    - #336: all five items. An exit that moves from 0 to 78 in the issue's
      listed cells is approved, because it reports a broken state. The
      marketplace is young and barely used, so judge it less strictly than
      the lock: record each cell, do not chase new variants beyond the five.
    - #305: an emptied container that holds a comment is kept; one without
      a comment is removed whole, as today. Note it in the upgrading guide.
    - #312: the behaviour is already fixed (`grim --global install --force`).
      Add the global-scope test and keep the project text byte-identical.
    - #306: first establish whether Claude Code reads a BOM-prefixed
      settings file, and record the answer in `.agents/upstream-watchlist.md`.
      Strip the BOM only if it does.
  - **Order.** The eleven small fixes land first. #335, #333 and #341 follow.
    #336 lands last on its own commits, so the PR stands without it if it
    does not converge inside its inner cap; what is unproven then stays on
    the issue with its cells, and the DONE block says so.
  - **Reviews.** At most 2 /hex-review calls. A review reports convergence
    per area. A finding outside the 16 issues becomes a follow-up issue and
    holds no criterion unmet.
  - **Docs last.** One docs and records pass after the last code fix lands,
    so pages, the upgrading guide, schema text and the catalog skills
    (`catalog/README.md` drift duty) match the final code.
  - **Worktrees and builds.** At most 3 worker worktrees, edit-only, sharing
    one target directory. The full gate runs only in the one integration
    worktree. Whoever creates a worktree removes it.
  - **Gate command.** `env -u OCX_GLOBAL task --force verify`.
  - **Windows.** This host has no Windows machine: the local `grim-wintest`
    harness is absent. A Windows claim rests on a `verify-deep.yml` run.
    Batch every Windows-relevant change before a dispatch.
  - **Models.** Sub-orchestrators and larger tasks follow the owner's model
    routing in `~/.claude/CLAUDE.md`; every spawn names its model and carries
    a `Model rationale:` line. Codex on Sol 6.1 is the cross-model adversary;
    a native seat substitutes only when Codex is unavailable, and the report
    says so.
  - **Other repositories.** A PR in another grimoire-rs repository opens
    only after this PR is ready, stays small, and says in its body that the
    grim PR merges first.
  - **DONE block.** It opens with a plain status before the criterion lines:
    what works; what is still broken, in user terms; how far from done; the
    decision needed. Every issue, PR and commit is a full link. Security
    items appear only by location and class, in one short list.

## Emphasis

- Fifteen of the sixteen issues are independent and small. Keep them that
  way: no shared refactor, no drive-by cleanup, one reviewable commit each.
- #336 is the only large item and the area that produced a new variant in
  each of reviews 8 to 10 of the lock-pin run. Items 2 and 3 are untraced
  and need the grim 0.15.1 binary (`task test:old-grim`).
- #342 leaves 11 failures, all test-side: `newline="\n"` at the four Codex
  rules-file writes, `as_posix()` for the catalog-drift absolute path,
  `encoding="utf-8"` in `test_schema_is_additive`, and the Warp round-trip
  expecting its Windows decline. More may surface once those pass.
- #311 was inferred from the call graph and never reproduced. Reproduce it
  before fixing it.

## Context

- Triage and decisions: `.claude/state/issue-sweep-decisions.md` (gitignored;
  its decisions are copied into § Rules above, which is authoritative).
- Issues: https://github.com/grimoire-rs/grimoire/issues/342, /341, /338,
  /336, /335, /333, /317, /316, /315, /312, /311, /306, /305, /301, /300, /299.
- #342's remaining failures:
  https://github.com/grimoire-rs/grimoire/issues/342#issuecomment-6753135317
- #336's cells: `.agents/plans/plan_lock_pin_stability.md`, § Principle 9,
  row 25 and its residuals.
- Trunk: `main` at 1dbf5ac6, which carries
  https://github.com/grimoire-rs/grimoire/pull/339.
