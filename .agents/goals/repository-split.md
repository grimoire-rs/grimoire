# Goal: "Repository split for the mirror tool and a GUI application"

Source: .agents/discussions/repository-split.md · Written: 2026-11-25 by /hex-loop

## Definition of done

- [ ] "grim-mirror repo holds the crate and its ten suites" — evidence: the commit, PR comment or
  artifact that satisfies it, for bullets 1 and 11 of the source's `## Requirements`.
- [x] "#398 rewritten without the mirror crate" — evidence, for bullet 2 of the source's `## Requirements`: branch `hex/repository-split` carries the nine commits of pull request 398 and removes `crates/grim-mirror` (commit ef5710d7, before /hex-finalize rewrites it). Plugin dispatch, the `mirror` table and `grim convert` stay. `task --force verify` is green with 4601 acceptance cases, and review call 2 approved at 31a8f663. Difference, per GD-2: the rewrite sits on this branch and its own pull request; nothing was pushed to `hex/artifact-mirroring`.
- [ ] "grim-sdk in #398, pinned by both new repos" — evidence: the commit, PR comment or
  artifact that satisfies it, for bullets 5 and 6 of the source's `## Requirements`.
- [ ] "grim-gui Tauri 2 skeleton starts as grim gui" — evidence: the commit, PR comment or
  artifact that satisfies it, for bullets 3 and 12 of the source's `## Requirements`.
- [x] "Nothing links grim's code" — evidence, for bullet 4 of the source's `## Requirements`: the local repositories `grim-mirror` and `grim-gui` depend on `grim-sdk` only and hold no `grimoire::` use; `crates/grim-sdk` runs the binary found through `GRIM_BIN`, then `PATH`, with `--format json`. Review call 2 confirmed bullet 4 at 31a8f663.
- [x] "Scaffold copied with a manifest, bundles first" — evidence, for bullets 7, 8 and 9 of the source's `## Requirements`: `tools/scaffold_manifest.py check` exits 0 in both local repositories (grim-mirror commit 1a476f8, grim-gui commit 053d944); each `grimoire.toml` installs the rule and skill bundles; taskfiles, workflows, cargo and test-harness config are the same scaffold. Review call 2 confirmed bullets 7 to 9.
- [x] "Agent config and testing idea adopted in full" — evidence, for bullets 9, 10, 13, 14 and 15 of the source's `## Requirements`: each manifest holds one entry per inventory row and its checker exits 0; `AGENTS.md`, `.claude` rules, hooks and structural tests are adapted per repository, with the commit guard and its root task. Gates after the fix pass: grim-mirror 236 unit and 112 acceptance cases with a suite floor, grim-gui 15 unit, 8 page and 43 acceptance cases. Review call 2 confirmed bullets 9, 10 and 13 to 15.
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

- Entry point: Run the /hex-plan skill on "Repository split for the mirror tool and a GUI application, per .agents/discussions/repository-split.md".
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

- Source: .agents/discussions/repository-split.md. Its `## Requirements` has 15 bullets, merged here into seven criteria to fit the prompt budget; each criterion names the bullets it covers and is met only when every one of them is. The full text, the decisions, the rulings open to the owner's veto, the open questions and the verification list are there.
- Research: .agents/research/research_repository_split_recon.md, .agents/research/research_repository_split_prior_art.md, .agents/research/research_repository_split_ocx.md, .agents/research/research_repository_split_conventions_template.md, .agents/research/research_repository_split_agent_config.md, .agents/research/research_repository_split_testing_and_conventions.md
- Swarm memory: .agents/memory/hex.md
- Pull request that holds the mirror crate today: https://github.com/grimoire-rs/grimoire/pull/398 on branch `hex/artifact-mirroring`. The grimoire part of this goal changes that pull request; the source's Decisions give the order of the steps.
- Follow-up issues about the mirror tool: https://github.com/grimoire-rs/grimoire/issues/399, https://github.com/grimoire-rs/grimoire/issues/400, https://github.com/grimoire-rs/grimoire/issues/401, https://github.com/grimoire-rs/grimoire/issues/402
- Earlier work: .agents/discussions/artifact-mirroring.md, .agents/adr/adr_artifact_mirroring.md, .agents/plans/plan_artifact_mirroring.md, .agents/goals/artifact-mirroring.md
- Decisions recorded under § Issue resolution, by the meta-orchestrator at the start of the run (2026-11-26):
  - GD-1, authority for the two new repositories. Question: the owner said in the discussion that the session may create the two repositories and push to them, but the pasted prompt reads "Also granted: none" and rules that a grant not quotable from the paste is omitted. Decision: the paste wins. Both repositories are built as local git repositories at `/home/mherwig/dev/grimoire-rs/grim-mirror` and `/home/mherwig/dev/grimoire-rs/grim-gui`, with every gate run locally. No repository is created on GitHub and nothing is pushed there. A criterion that needs the remote repository is reported `not met` with the one act that closes it, unless the owner grants that act in a message during the run.
  - GD-2, where the grimoire part lives. Question: the paste names branch `hex/repository-split`, created from the trunk, and grants a force-push only to that branch, while the source asks to rewrite pull request 398 on branch `hex/artifact-mirroring`. Decision: `hex/repository-split` starts at the trunk, takes this file as its first commit, then the nine commits of `hex/artifact-mirroring` (tip `eaa7da0b`) by cherry-pick. The removal of the mirror crate and the new `crates/grim-sdk` land on it. Its one pull request carries the rewrite and names pull request 398 as the one it replaces. Pushing the same tip to `hex/artifact-mirroring`, or closing 398, stays the owner's act. The criterion "#398 rewritten without the mirror crate" is evidenced by the content of the new pull request and reported with that difference stated.
  - GD-3, the pin of `grim-sdk` in the two new repositories. The commit both repositories pin must exist on GitHub, and /hex-finalize rewrites this branch before it pushes. Decision: during the run both repositories reach `grim-sdk` through a local path override; the pinned commit is written after /hex-finalize has pushed `hex/repository-split`, as the last local commit in each new repository.
