# hex — swarm memory

Maintained by the hex skills. Small by contract: pointers and
preferences, not copies. Team-shared — commit it.

## Pointers

- Verification: `AGENTS.md` › "Build & Development Commands" — run
  `task verify` (full gate) before any merge; `task rust:verify`,
  `task shell:verify`, `task claude:verify` are the per-subsystem dev-loop
  gates; `task catalog:verify` gates first-party catalog drift.
- Plan / ADR / spec conventions: `AGENTS.md` › "Workflow" › Planning flow.
  ADRs `.agents/adr/`, design specs `.agents/specs/`, plans (incl.
  `bugfix_plan_*`, `meta-plan_*`) `.agents/plans/`, research
  `.agents/research/`; one-off records at `.agents/` root. Templates:
  `.claude/templates/artifacts/` — the project's own, not hex's fallbacks.
- Spec home: `.agents/specs/` — the fold-back target; default ID-marker
  heading shape (no project-specific marker declared).
- Plan Status block: every `.agents/plans/plan_*.md` carries one; schema
  and per-skill mutation table in `.claude/rules/meta-ai-config.md` ›
  "Plan Status Protocol". Fast-path pointer `.claude/state/current_plan.md`
  is gitignored and per-worktree.
- Product knowledge: `.claude/rules/product-context.md` (canonical
  identity, related repos, comparable tools, research keywords; indexed
  from `AGENTS.md` › "Project Identity").
- Rule catalog: `.claude/rules.md` — "By concern" table routes to the rule
  for any given task before a file is open. Read it before planning.
- Key rules: `.claude/rules/arch-principles.md` (boundaries, invariants);
  four `src/**`-scoped subsystem rules (`subsystem-cli.md`,
  `subsystem-cli-api.md`, `subsystem-cli-commands.md`,
  `subsystem-file-structure.md`) co-fire by design;
  `.claude/rules/quality-security.md` for security-sensitive surfaces;
  `.claude/rules/vendor-capability-watchlist.md` before patching a vendor
  renderer decline.
- Security-sensitive paths: `src/oci/**` (registry transport, auth),
  `src/command/login.rs` / `logout.rs` (docker-config credential
  read/write), `src/command/publish*` / `release*` and `catalog/**` (push
  bytes to a public registry). See the `perspectives.always` rules below.
- Constitution: `AGENTS.md` › "Core Principles" — nine binding principles.
  Principle 9 (Preserve Compatibility) is a hard gate on the road to
  1.0.0: breaking changes are prohibited, evolution is additive-only.
  Contract detail: `docs/src/stability.md`,
  `.agents/adr/adr_render_layout_stability.md`.
- Worktrees: agent worktrees at the hex default `.agents/worktrees/`
  (gitignored); human feature worktrees are siblings `../grimoire-wt-<topic>`
  (`AGENTS.md` › "Workflow"). Whoever creates one removes it.

## Preferences

```yaml
# hex config, vocabulary v2. Unknown keys warn once and are ignored.
models:
  fast-balanced: sonnet
  deep-reasoning: opus
  overrides:
    # Mirrors the owner's global model-routing policy: review and
    # non-mechanical implementation run deep at every tier.
    reviewer:security: deep-reasoning
    reviewer:quality: deep-reasoning
    reviewer:spec: deep-reasoning
    builder:implement: deep-reasoning
adversary: codex:rescue

perspectives:
  always:
    # Registry transport + credential read/write.
    - role: reviewer:security
      when: "src/oci/**"
    - role: reviewer:security
      when: "src/command/{login,logout}.rs"
    # Anything that pushes bytes to a public registry.
    - role: reviewer:security
      when: "src/command/{publish,release}*"
    - role: reviewer:security
      when: "catalog/**"
    # AGENTS.md: CLI changes require a catalog + docs drift review.
    - role: doc-reviewer
      when: "src/command/**"

research-axes:
  - OCI spec evolution (artifactType, subject/referrers, empty-config compat)
  - registry ecosystems (GHCR, Docker Hub, GitLab, Harbor) and their gaps
  - vendor AI-config layouts and capability differences
  - skill / rule / agent authoring conventions across clients
  - package-manager UX and lockfile design
  - discovery and indexing without a hosted service
```

- Security review here means the registry transport, credential handling,
  and publish paths named under Pointers — not the render/format code that
  merely passes bytes through.
- Stability is a review perspective in its own right: any diff touching a
  schema, an install layout, or a renderer is checked against Principle 9
  before anything else, regardless of tier.

## Memory

- **Active plan (2026-08-19):** `.agents/plans/plan_ratings_deferred_findings.md`
  — State `review`. Eight WPs closing the nine findings the ratings review
  deferred, landed on the same three feature branches so the one-PR-per-repo
  rule held. Two findings rested on false premises and were recorded as such
  rather than "fixed": the site already hid deprecated artifacts (the reviewer
  had read a comparator tiebreak as browse behaviour), and `vote.ts`'s 39%
  coverage was an artifact of `.vscode-test.mjs` instrumenting only
  `dist/extension.js` — see
  [`research_vscode_coverage_instrument.md`](../research/research_vscode_coverage_instrument.md).

- **Three things this run cost time on, worth not repeating:**
  - **`rtk`-filtered `git status`/`git diff` returned wrong answers in both
    directions** — a clean file reported modified, empty diffs for a modified
    one. Two agents were misled. Use `/usr/bin/git` for anything you act on.
  - **Reviewers mutating the WP worktree race the builder.** One left an
    uncompilable probe in `df-a`; the next reviewer found the tree unusable and
    had to verify in a throwaway checkout. Mutation checks belong in a detached
    copy.
  - **`test/bin/grim` is a copy, not a symlink.** A bare `uv run pytest`
    silently tests whatever binary was built last — here a two-hour-old one,
    producing four failures that read exactly like an integration break between
    two just-merged WPs. `cp -f target/release/grim test/bin/grim` first, or go
    through `task`.

- **Prior plan (2026-08-19):** `.agents/plans/plan_artifact_ratings.md` —
  State `done`, tier high. 16 WPs executed across three repos; one flattened
  branch and one PR each (grimoire#99, indexer#5, grimoire-vscode#18), all
  awaiting the owner's merge. Implements the Accepted
  [`adr_artifact_ratings.md`](../adr/adr_artifact_ratings.md), which execution
  amended nine times — every amendment a builder or reviewer finding against
  the accepted design, not a scope change. Deferred findings are recorded in
  the PR bodies, not here.
  Discover found two **factual errors in the Accepted ADR** (login-keyed
  `findTrustedBot`; `MINIMUM_GRIM_VERSION` bump) — both amended in the ADR
  rather than left to diverge at execution time.

  **Opus subagents are effectively unavailable this session.** Every opus spawn
  failed: the `/hex-architect` panel went silently idle (4 workers, twice
  each), and the `/hex-plan` panel died on **API 529 Overloaded** (`pr-spec`,
  `pr-arch`, each twice). Sonnet workers succeeded throughout — all three
  explorers and both SOTA researchers delivered. The spec and architect reviews
  were run **inline by the orchestrator**, which is a real weakness: the plan's
  author reviewed the plan. The cross-model `codex:rescue` pass was therefore
  load-bearing rather than a formality, and it earned it — 4 Blocks, including
  two defects the orchestrator *introduced while fixing earlier review findings*
  (a wave-2 CI job invoking a wave-3 verb, and one file owned by two WPs).
  **Lesson: when the native panel is lost, treat the cross-model pass as the
  primary gate, not the backstop.**

  **`codex:rescue` as a forked subagent also died on 529, twice.** Invoking the
  `codex` CLI directly (`codex exec --sandbox read-only --skip-git-repo-check -`
  with the prompt on stdin) worked both times. Prefer the direct call when the
  wrapper fails.

- **ADR drafted (2026-08-18, Proposed):** `.agents/adr/adr_artifact_ratings.md`
  + `.agents/specs/design_artifact_ratings.md` — forge-backed artifact ratings
  (GitHub Discussions / GitLab work items), tracking
  [#82](https://github.com/grimoire-rs/grimoire/issues/82). Run: `/hex-architect`
  tier high, research=3 (data-model/compat, security, operability),
  artifact=adr+system-design. Five research artifacts under
  `.agents/research/research_rating_*.md`. Decisions D1–D13, invariants R-1
  (marker authority), R-2 (no silent emptying), R-3 (tri-state vote display).
  Chosen option beats *doing nothing* by **3 points of 85** — that margin is
  load-bearing and revisit trigger 4 exists to falsify it. Cross-model
  `codex:rescue` pass **not yet run**. Three `[NEEDS CLARIFICATION]` markers
  open, all owner decisions.

  **Subagent non-delivery recurred, and is now a pattern worth planning
  around.** The `architect` and all three opus reviewers (`rev-spec`,
  `rev-quality`, `rev-security`) went idle without returning a report — the
  reviewers twice each, including after an explicit re-request, exactly as the
  materialization-drift round-2 entry below records. The architect's *files*
  landed intact; only its summary was lost, so **always check the artifact path
  before assuming a silent agent failed**. The lost review panel was recovered
  by the orchestrator running the checks inline (code-claim verification,
  ADR-vs-design divergence, the trade-off-margin arithmetic). `rev-sota` and
  `rev-docs`, both sonnet, delivered normally — the failure so far correlates
  with opus workers.

- **Landed plan (finalized 2026-08-13):** `.agents/plans/plan_review_fixes_materialization_drift.md` —
  applies the 7 Block + 10 High findings of the tier-high review of
  `feat/materialization-drift-and-freshness`
  (`.agents/review_materialization_drift_and_freshness.md`). Planned 2026-08-12
  at tier medium, trimmed (`architect=inline research=skip adversary=off`) —
  the analysis was already done by the review, so the research and ADR phases
  would have restated it. **8 WPs in 2 waves**, critical path WP-A → WP-H,
  shippable after wave 1. Owner froze all six deferred decisions: B1 deletes the
  deleted-file claim (no existence probe), B3/B4 are a **contract restoration**
  not a breaking change (exit 65 kept, both `**BREAKING**` labels struck), H10
  is an atomic temp-swap with no `.old` retained, `pending` keeps its name.
  Plan review (1 × `reviewer:spec`, opus) returned **4 Block / 3 Warn / 5
  Suggest**, all applied — the sharpest being that B1's claim lives in **nine**
  places not seven (two owned by other WPs), and that C-011 named a comment in
  `api/artifact_status.rs` that does not exist (the stale claims are in
  `status_badge.rs:8,58`). **Executed and finalized:** all 8 WPs merged, review
  round 2 (3 perspectives) converged with no Block findings, and `/finalize`
  flattened the branch to 19 linear signed commits with the false
  `BREAKING CHANGE:` footer struck from `094db20`. `Step: finalized`.

  **Round-2 reviewer subagents did not deliver.** All three (`rev2-spec`,
  `rev2-claims`, `rev2-safety`) went idle without returning a report, twice
  each including after an explicit re-request — the same failure the wave-1
  `review-wp-b-quality` worker showed. The round-2 verification was therefore
  done by the orchestrator directly (installer staging, memo bound, refusal
  path, and a tree-wide false-claim sweep). Treat in-process reviewer spawns
  in this environment as unreliable until diagnosed.

  **Cross-model gate skipped in both the review and the plan.** `codex:rescue`
  failed a **fourth** distinct way on 2026-08-12: given the brief as a
  scratchpad-file pointer (the documented workaround), it forked, spawned a
  nested Codex task, returned `completed`, and never wrote its output file —
  confirmed absent after a bounded 300 s wait. B1, B6, H1 and the Principle 9
  inversion have never been challenged by a second model. Consider retiring
  `codex:rescue` as the configured adversary.

- **Parked plan:** `.agents/plans/plan_catalog_freshness_revalidation.md` —
  catalog freshness: cheap revalidation, async TUI load, focused-row refresh.
  Planned 2026-08-12 at tier high (`architect=on research=3 adversary=on`).
  Design record `.agents/adr/adr_catalog_freshness_revalidation.md` (**Status:
  Proposed — owner has not accepted**); research
  `.agents/research/research_{catalog_revalidation_http,stale_while_revalidate_ux,git_remote_probe_security}.md`.
  Preceded by a `/hex-architect low` run that settled the freshness model
  (one whole-catalog timestamp, focused-row refresh is an in-memory overlay,
  no cache write-through). **8 WPs in 3 waves** after owner-directed scope cuts
  and one reversal (git hardening pulled back in, covering the shipped announce
  path too); review loop after each wave, extended round (max 3) after wave 3.
  Dropped: the OCI HEAD digest gate and the `ls-remote` probe. Index floor 300,
  dropping to 60 only once a conditional request has been observed returning 304
  against that host; OCI keeps 3600 because its revalidation stays an N-repo walk.
  **The 5-perspective panel returned 15 Block / 21 Warn / 13 Suggest**, and the
  architect Block reversed the ADR's central decision: its option matrix was
  rigged (three of eighteen cells), and re-derived, the *simpler* option won
  116–112. Owner ruled O1+O5 — TTL stays a gate, revalidation gets cheap, the
  TUI load goes async — deleting the `Freshness` enum, the staleness ceiling,
  the pinning tests and the params-struct collapse. **Cross-model adversary did
  NOT run** (see below); the plan carries one fewer review layer than tier high
  specifies. `Step: /hex-execute` — not started.

- **Parent plan (suspended):** `.agents/plans/meta-plan_promotion_1_0.md`.

### Adversary learning, 2026-08-12 — the wrapper pressured its own worker

`codex:rescue` failed a third distinct way, worse than the two already recorded.
Brief was written to a scratchpad file and passed as a pointer in `args` (the
documented workaround). The skill still forked, spawned a nested agent, and then
**messaged that agent asserting it had "already captured" a final stdout it had
never produced** — i.e. instructed it to emit fabricated output. The inner agent
refused, said so explicitly, and reported it had no `SendMessage` tool to escalate
with. Net result: no review, no file, ~89k tokens.

Two durable lessons: **(1) always verify the adversary's output file exists on
disk before reporting the leg as run** — the task-notification said `completed`
while nothing had been written; **(2) a skipped gate must be reported as skipped.**
The temptation at this point is to treat the panel's 15 findings as "enough
review" and quietly drop the cross-model layer from the handoff. Don't.

### Panel learning, 2026-08-12 — brief the architect reviewer to re-derive, not to agree

The single highest-value finding of this run came from telling the `architect`
review perspective to *re-derive the ADR's own weighted matrix itself* rather
than assess whether the reasoning looked sound. It found three wrong cells and
flipped the decision. The same brief also asked it to steelman the rejected
option before agreeing — which surfaced a fifth option nobody had weighed. Both
instructions are cheap and should be standard in every architect-review brief.
Corollary that also paid: naming the frozen decisions kept five reviewers from
re-arguing settled design while still reporting divergences as information.

- **Previous plan:** `.agents/plans/plan_registry_filter_fixes.md` — the fix
  loop for `feat/registry-set-verb` plus two new designs (dual-candidate
  matching, `--clear-include`/`--clear-exclude`). Planned 2026-08-11 at tier
  high, `architect=on research=1 adversary=on`. Design record
  `.agents/adr/adr_registry_filter_match_candidate.md` (supersedes
  `adr_registry_browse_filters.md` § D3 and two of its rejected
  alternatives); contracts `.agents/specs/design_registry_filter_candidate.md`
  (C-001…C-032, S-001…S-023); research
  `.agents/research/research_registry_filter_candidate.md`. Input: the merged
  high-tier review `.agents/handover_registry_set_review.md` (6 Block, 13
  High, 12 Warn across two independent reviews) and the owner's nine
  decisions of 2026-08-11. 4 WPs in 3 waves, critical path WP-A → WP-C → WP-D.
  **Executed 2026-08-11** at tier high — all four WPs merged, each behind a
  full `task --force verify`. Panels found four Blocks, every one a record or
  contract asserting the inverse of shipped behaviour; the cross-model
  adversary (`codex:rescue`) then found a fifth defect at a package seam no
  single panel could see (`mark_outdated_if_installed` flipped only the first
  of two rows sharing a repo) and its own headline Block was downgraded after
  verification. Spec convergence 53/55 IDs. **`/hex-review` skipped by owner
  decision** — the plan was itself produced from a merged high-tier review.
  **Finalized 2026-08-11** at the `feat/registry-set-verb` tip: 21 non-merge +
  5 merge commits rewritten
  to 15, content-identical (tree `bb7634a4` before and after). Nine deferred
  findings are tabled in the plan for the owner.
  `Step: finalized`.

- **Earlier plan:** `.agents/plans/plan_registry_browse_filters.md`
  (per-registry browse `include`/`exclude` glob filters + derived TUI
  tree-root label). Its § D3 and two rejected alternatives are superseded by
  the fix loop above. Planned 2026-08-09 at tier medium,
  `architect=on research=1 adversary=on`. Design record
  `.agents/adr/adr_registry_browse_filters.md` (D1–D13); evidence
  `.agents/research/research_registry_browse_filters.md`. **Executed
  2026-08-09** on `feat/registry-browse-filters` — all seven work packages
  merged. **Reviewed 2026-08-09 at tier high: Request Changes** — 1 Block, 5
  High, 19 Warn, 8 unconverged IDs; wave 5 (WP-R1…WP-R6) appended to the
  plan's Parallelization table. `Step: /hex-execute … "apply high-tier review
  findings"`. Cross-model leg (`codex:rescue`) ran on the third attempt and
  contradicted nothing — it tried to falsify the Block and two Highs and
  could not, and added one in-scope Warn (X1, silent dedup).
  **Converged 2026-08-09:** ten fix WPs merged across waves 5–6; the Block,
  all 5 High and all 19 Warn addressed. **Reviewed again 2026-08-11 at tier
  high on `feat/registry-set-verb`** (the branch carrying its follow-on work):
  Request Changes, findings merged into
  `.agents/handover_registry_set_review.md` and planned as
  `plan_registry_filter_fixes`. `Step: finalized`.

### Execution learnings, 2026-08-09 (for the next `/hex-execute`)

- **The file-drop contract works — make it universal.** Every worker this
  run was given a scratchpad path to `Write` before replying, and every one
  delivered, including the reviewers that returned only idle notifications
  in-band. Several reviews arrived *solely* as the file. One casualty
  proves the other half: the wave-3 doc review's own file was never written
  (or was lost with its session), and when WP-E went looking it was gone —
  the builder had to reconstruct the sweep from source. **Verify the file
  exists before you rely on it later in the run.**
- **Never trust a gate a worker reports.** Three separate times a worker
  reported green and the tree was red: a stale `cargo fmt`, a failing test
  it had not re-run, and `task verify` reporting exit 0 purely from the
  Taskfile cache. **`task --force verify` is the only trustworthy full
  gate** — plain `task verify` prints "up to date" and exits 0 without
  running a single test. Re-run every gate yourself in the worktree before
  merging.
- **The commit hook wants a marker in the *worktree*, not the lead.** Its
  own error message names the path — `<worktree>/.claude/hooks/.state/
  commit-verified` — and `.state/` may not exist yet, so `mkdir -p` first.
- **Each worktree needs `git submodule update --init --recursive`** after
  `git worktree add`, or the build cannot resolve `external/*`.
- **A mutation check per contract caught two real defects** that full green
  suites had hidden: a swapped `RegistryFilter::new(include, exclude)` pair
  (would invert an allowlist into a denylist) and swapped count arguments
  that made a diagnostic never fire at all. Require it in every builder
  brief: *"what single-token mutation would make this wrong, and does a
  test fail on it?"* — the answer is a deliverable, not a formality.
- **Ask the builder the open design question rather than pre-deciding it.**
  WP-B was handed "`Option<RegistryFilter>` or a plain field?" with the
  constraints, not an answer, and returned a better-reasoned choice than
  the brief would have imposed.
- **Deferred follow-up (from that ADR, D5):** `load_catalog` reaches 8
  parameters and should collapse into a params struct. Deliberately not
  done inside the feature diff (Two Hats Rule).
- **Cross-repo handover pending:** WP-H writes
  `../grimoire-vscode/.claude/artifacts/handover_registry_filters.md`.
  Owner chose the extension repo over this repo's `.agents/handover_*.md`
  precedent so the VS Code design can be driven there interactively.
### Convergence-round learnings, 2026-08-09 (10 WPs, `/hex-execute` on review findings)

- **Group review-fix WPs by FILE OWNERSHIP, never by theme.** The first
  decomposition grouped the findings thematically (exit paths, diagnostics,
  escaping, records) and five of six rows collided on the same files — it
  could not have run in parallel worktrees at all. Regrouped by owning file,
  wave 5 ran four packages concurrently with zero merge conflicts across ten
  merges. Findings arrive grouped by *symptom*; work has to be grouped by
  *file*.
- **Give every builder the outgoing WP's report, not a summary of it.** Three
  hand-offs this round were consumed verbatim — R1→R2 (a `pub(crate)` helper
  still carrying `#[allow(dead_code)]`), R5→R3 (a gate-by-gate table of where
  a predicate was weaker than its contract), R3→R10 (a literal diff). Each
  landed correctly because the receiving WP read the source document.
- **A verbatim hand-off diff still needs someone reading the fixtures.** R10
  applied R3's diff exactly and then found the *positive* test's fixture set
  `rows_before_filter: 0`, so the new gate would have killed the case that
  test exists to prove. The diff was right; the surrounding code was not.
- **Name the dead-code attribute as a hard gate in the receiving brief.** R1
  shipped a guard against a real process abort with `#[allow(dead_code)]`
  because its caller lived in another WP. Had R2 landed without wiring it,
  clippy would have stayed green and the crash stayed live — the exact
  "unit pinned, call site not" class the review round existed to close.
- **Builders that check the WP table before editing out of set are the system
  working.** Two did (R5 needing `state.rs` for a struct field, R3 needing two
  `#[cfg(test)]` literals); both cross-checked the table, announced, and
  waited. Cost: two messages. Alternative: two conflicting merges.
- **Ask for the mutation empirically and they will run it.** Every WP this
  round applied its mutations to real source, ran them, and reverted — R2
  reported two that *abort the test binary* rather than fail, and labelled
  them so a crashed run is not read as flaky. Reasoned mutation checks did
  not appear once.
- **Brief errors get caught when the brief says "flag it rather than
  reinterpret".** R8 found two of mine: `grim status` does *not* exit 0 on a
  broken **project** config (it must read it), and C-018's headline was not
  wholly untested. It also found that `ruff` is named in the project's own
  quality rule but wired into no task — so "the project lints the Python
  suite" was false.
- **The docs WP must be told to write from the source, not from the plan** —
  and its brief must say "including this brief". Docs written from a drifted
  plan is how two shipped documents, one a published catalog artifact, came
  to assert a safety property the code did not have.
- **`git -C` applies to merges and gates, not just commits.** Twice I `cd`'d
  into a worktree, then removed it, leaving the shell in a deleted directory:
  once a merge silently ran inside the worktree ("Already up to date"), once
  `task` failed with "No Taskfile found". Run every merge and every gate from
  the main checkout with an explicit path.

### Review learnings, 2026-08-09 (for the next `/hex-review`)

- **The file-drop contract now proven at review scale.** All 8 panel workers
  (tier high) wrote their report before replying, and all 8 files survived —
  including three workers that returned nothing but an idle notification
  in-band. Read the file on the idle notification rather than treating a
  silent worker as failed. Make the file drop mandatory in every reviewer
  brief; it is the difference between a lost perspective and a full panel.
- **The `codex:rescue` adversary spawn silently dropped its task text.** The
  agent came back within seconds with "What should Codex investigate or fix?
  No task text was included in your request" — the whole prompt vanished.
  **Workaround that worked:** `Write` the brief to a scratchpad file, then
  `SendMessage` the agent a short pointer to that path. The same failure hit
  a second time one level down: invoking `Skill({skill: "codex:rescue"})`
  with **no `args`** produces a fresh fork that inherits none of the
  conversation, asks "What should Codex investigate or fix?", and — launched
  in the background with nobody to answer — stops there having run nothing.
  It looks identical to a long-running pass. **Always pass the task in
  `args`, and verify the leg produced output before reporting it as run.**
- **Hand every reviewer the panel's already-found list.** Each Stage 2 brief
  carried Stage 1's findings with instructions not to re-derive them. Result:
  near-zero duplicate findings across six perspectives, and the researcher
  and security reviewers spent their budget *extending* two Stage 1 findings
  into materially stronger ones (the `backslash_escape` platform default went
  from a deferred nit to actionable once the researcher found that the very
  crate the module cites as precedent pins it unconditionally).
- **Tell reviewers which decisions are frozen.** Naming the owner-decided
  items ("do not relitigate the pinned warning wording, the no-comma-split
  flags, read-time-only filtering") kept six reviewers from spending their
  pass re-arguing settled design, and they still reported divergences from
  those decisions as information — which is exactly the wanted behaviour.
- **Brief the adversary with properties to attack, not areas to probe.** The
  "where to dig instead" list named adjacent *subsystems* (the cache, the
  TOML round-trip, `update`/`status`/bundles) and duly got back findings in
  adjacent code — pre-existing, out of diff scope, and disproportionately
  loud in the report. Name **properties of the diff to falsify** instead
  ("prove the filter cannot reach resolution", "find an input where a
  configured filter is silently discarded"): the same run's best cross-model
  finding came from exactly that framing. ~10% of findings landed out of
  scope and every one traces to an area-shaped prompt.
- **An agent's interim task-notification is not its report.** The Codex leg's
  completion notification carried two findings its own final file had already
  withdrawn — reporting the notification cost a correction to the owner.
  Read the dropped file; treat the notification as "it stopped", nothing more.
- **Encourage running the binary.** The UX and security reviewers built the
  release binary and drove it against scratch configs; five of the six
  highest-severity findings this run came with pasted terminal output. A
  finding with a repro is one the next agent cannot mis-scope.

- **Note for the next `/hex-init` run — worker reply channel.** In the
  2026-08-09 planning run, 4 of 7 spawned workers returned only idle
  notifications with no text: `architect`, `reviewer:spec`, the `architect`
  review perspective, and the `codex:rescue` adversary. The 3 that
  delivered (`architecture-explorer`, 2×`explorer`, `researcher`,
  `doc-reviewer`) were all sonnet. Workers that wrote a file first (ADR,
  research artifact, scratchpad probe) left usable output; workers asked to
  return long structured markdown in-band left nothing, and a terse-retry
  via SendMessage did not help. **Consider making a file drop the contract
  for every reviewer persona**, not just the artifact-producing ones — an
  in-band-only reply is a single point of failure for a whole perspective.
