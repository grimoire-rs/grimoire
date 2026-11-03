# Goal: Declared dependencies between artifacts, resolved recursively through the bundle machinery

Source: .agents/adr/adr_artifact_dependencies.md · Written: 2026-11-02 by /hex-loop

## Definition of done

- [x] D1 carrier: requires layer as layers[1]; bundles and dependency-free artifacts byte-identical — evidence: the commit, PR comment or
  artifact that satisfies it.
- [x] D2 forward compat: F4 layout refusal + requires-features annotation, exit 65 — evidence: the commit, PR comment or
  artifact that satisfies it.
- [x] D3 resolution: recursive with caps, same-digest diamond merge, conflict 78, cycle 65 — evidence: the commit, PR comment or
  artifact that satisfies it.
- [x] D4 lock and removal: requires edges, stale-lock check, closure-aware effective set, hook consent — evidence: the commit, PR comment or
  artifact that satisfies it.
- [x] D5 why system: grim why, status/add/describe fields, TUI Required by row — evidence: the commit, PR comment or
  artifact that satisfies it.
- [x] D5 grimoire-vscode shows Required by — evidence: the commit, PR comment or
  artifact that satisfies it.
- [x] D6 every kind declares requires in source, one grammar; nested bundles — evidence: the commit, PR comment or
  artifact that satisfies it.
- [x] D7 export flattens the closure — evidence: the commit, PR comment or
  artifact that satisfies it.
- [x] D8 hardening: shared registry_text, validated generated_by, publisher named in errors — evidence: the commit, PR comment or
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

- Entry point: Run the /hex-plan skill on "Declared dependencies between artifacts, resolved recursively through the bundle machinery, per .agents/adr/adr_artifact_dependencies.md".
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

- ADR: .agents/adr/adr_artifact_dependencies.md (Status: Proposed; § Open questions holds 2 items, each with a recommendation)
- Design: .agents/specs/design_artifact_dependencies.md
- Discussion: .agents/discussions/artifact-dependencies.md
- Research: .agents/research/research_artifact_dependencies_*.md (8 artifacts)
- ADR § Migration and rollout: release gates for tagging 0.16.0 (five-registry push, grimoire-vscode release, GRIM_OLD_BIN run, separate fix(resolve) fan-out commit); upgrade fixture test/data/golden/pre_dependencies_cdeedcaa/
- grimoire-vscode lives in a separate repo (../grimoire-vscode); the second D5 criterion needs a grant covering it.
- 2026-11-03: D1–D3 and D5–D8 (this repo) ticked after review round 3 (f0c74e2d): every pointer in the plan Goal DoD trace resolves, and the evidence is in .agents/.tmp/review3/spec.md. Gate green on 6e390b49: nextest 4882, acceptance 2256 passed / 60 skipped. D4 is left unticked because of a Block on its force scope, https://github.com/grimoire-rs/grimoire/issues/263. Follow-ups: https://github.com/grimoire-rs/grimoire/issues/264. grimoire-vscode: https://github.com/grimoire-rs/grimoire/issues/265. Loop stopped: the I8 cap (2 outer cycles) and the 3 reviews are spent.
- 2026-11-03: owner granted the remaining work (fix #263, the grimoire-vscode half, finalize). D4 ticked: #263 fixed in 2db4c653 + 22558444, review round 4 Approve/Converged (a8372e0c), gate green (nextest 4882, acceptance 2257 passed / 60 skipped). D5 grimoire-vscode ticked: Required by row, kept-by-dependency, pre-add requires note, dependency-only Uninstall/Update disabled — uncommitted on feat/dependencies-required-by in ../grimoire-vscode at the owner's request for review; npm run check 0, 1489 tests pass.
