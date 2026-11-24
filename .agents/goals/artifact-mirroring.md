# Goal: "Mirroring of artifacts into grimoire"

Source: .agents/discussions/artifact-mirroring.md · Written: 2026-11-24 by /hex-loop

## Definition of done

- [x] "Import artifacts out of a monorepo: one upstream repository yields many artifacts ('import like artifacts of [a monorepo…" — evidence: `test/tests/test_mirror_monorepo.py` against a real registry (`test_sync_publishes_one_artifact_per_unit`, `test_published_skill_payload_equals_the_upstream_file`); code in 8b2a95d0 and a5e75279; closing /hex-review Approve at dd6d0066.
- [x] "'Then we may want to bundle them': the mirror can publish bundles whose members are mirrored artifacts." — evidence: `test/tests/test_mirror_bundles.py` against a real registry (`test_declared_bundle_lists_members_as_absolute_exact_refs`, `test_grim_add_of_a_mirrored_bundle_installs_every_member`); code in a5e75279; closing /hex-review Approve at dd6d0066.
- [x] "'Mirror multiple rules and so forth': every non-skill kind an upstream carries is a candidate, not skills alone." — evidence: `test/tests/test_mirror_kinds.py` (`test_rules_and_agents_publish_with_their_own_kind`, `test_agent_named_differently_from_its_file_publishes_under_its_declared_name`) and `test/tests/test_mirror_dialect.py` (`test_claude_dialect_agent_publishes_as_an_agent_keeping_its_fields`) against a real registry; code in 8b2a95d0 and 03f9d28c. Rules and agents publish; hooks, MCP servers, settings and mods are refused and reported as skipped, as the source's Decisions and ADR `.agents/adr/adr_artifact_mirroring.md` record. Closing /hex-review Approve at dd6d0066.
- [x] "Harness marketplaces as an input 'maybe ... from the beginning'; the owner left that call to the research (see Decisions…" — evidence: `test/tests/test_mirror_marketplace.py` against a real registry (`test_registry_holds_each_member_with_its_own_origin`, `test_plugin_bundle_layer_lists_exactly_its_member_refs`, `test_grim_add_of_a_plugin_bundle_installs_its_members`, `test_external_plugin_bundle_names_the_repository_its_revision_is_in`); code in 8b2a95d0, 03f9d28c and a8075220; closing /hex-review Approve at dd6d0066.
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

- Entry point: Run the /hex-plan skill on "Mirroring of artifacts into grimoire, per .agents/discussions/artifact-mirroring.md".
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

- Source: .agents/discussions/artifact-mirroring.md
- Research: .agents/research/research_artifact_mirroring_adjacent.md, .agents/research/research_artifact_mirroring_archaeology.md, .agents/research/research_artifact_mirroring_prior_art.md, .agents/research/research_artifact_mirroring_recon.md, .agents/research/research_artifact_mirroring_repo_survey.md, .agents/research/research_artifact_mirroring_vendor.md, .agents/research/research_artifact_mirroring_version_signals.md
- Swarm memory: .agents/memory/hex.md
- Decisions recorded under § Issue resolution: .agents/plans/plan_artifact_mirroring.md § Decisions taken while planning (PD-1 is the repository-scope decision: the mirror is the workspace crate `crates/grim-mirror`, its own binary, because this run holds no grant for a second repository) and § Execution decisions (ED-1 to ED-15); .agents/adr/adr_artifact_mirroring.md (D1 to D19, Status Proposed).
- Reviews: three /hex-review calls. Call 1 on 896dcb6e..8743bca7, changes required. Call 2 on 8743bca7..63da3f77, changes required. Call 3 on 63da3f77..dd6d0066, Approve. Both refinement cycles are spent.
