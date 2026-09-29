# Goal: "Harness marketplace export — `grim export marketplace`, committed marketplace repo, render-derived plugin versions"

Source: .agents/adr/adr_harness_marketplace_export.md · Written: 2026-09-29 by /hex-loop

## Definition of done

- [x] "D1 — Marketplace repo generation model (central decision)" — evidence: `src/export/marketplace_export.rs` stateless regenerate; tests `test_s001`, `test_s002`, `test_s005` in `test/tests/test_export_marketplace.py`; live no-op regenerate run https://github.com/grimoire-rs/e2e-marketplace/actions/runs/36541838811.
- [x] "D2 — Repo layout, marketplace file set, client mapping" — evidence: Claude/Copilot/Codex/Qoder default, Cursor opt-in; `c013_the_table_is_adr_d2…`, `c005_manifest_rel_per_client`, `test_s019`, `test_s019b`, `test_s020`.
- [x] "D3 — Marketplace document" — evidence: `c019_document_bytes_are_fixed`, `test_s001_table_metadata…`.
- [x] "D4 — `[marketplace]` table in `marketplace.toml`" — evidence: `src/export/marketplace.rs`; `c008_*`, `c009_*`, `test_s010`, `test_s011`.
- [x] "D5 — `grim export marketplace` CLI" — evidence: `src/command/export.rs` `export marketplace`; `c012_*`, `test_c012_plain_output…` (Windows too); published dev build dev.ocx.sh/grimoire/cli:0.15.0-dev_20260929033644.
- [x] "D6 — Tree inventory, comparison, filesystem safety" — evidence: `c001_*`, `c015_*`, `c017_*`, `r222_*`, `test_s012_*`, `test_s013_*`, `test_s014`; Windows rig run: all export-marketplace tests pass incl. junction cases.
- [x] "D7 — Plugin `version` derivation" — evidence: `c002_golden_vector`, `c003_*`, `test_s006` (Linux + Windows rig).
- [x] "D8 — JSON report, errors, exit codes" — evidence: `src/api/export_report.rs`; `c034_*`, `c021_exit_codes_and_reasons_equal_main`.
- [x] "D9 — Maintenance job contract" — evidence: e2e repo https://github.com/grimoire-rs/e2e-marketplace (regenerate/verify/gate workflows green on main); GitLab component https://gitlab.com/grimoire-rs/components/-/merge_requests/2; guide `docs/src/content/docs/guides/hosting-a-marketplace.md`.
- [x] "D10 — Indexer changes (`grimoire-indexer`)" — evidence: https://github.com/grimoire-rs/indexer/pull/10 (`task check` 871 tests: enrich.test.ts, marketplace.test.ts, build.test.ts).
- [x] "D11 — Principle 9 gate; frozen contracts created" — evidence: `docs/src/content/docs/stability.md` + `upgrading.md` rows; `c021_*`, `test_s011`, `test_s019b`; `export plugin` default output change (Qoder) release-noted.
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
- Granted acts (authority: the pasted prompt): Make an e2e-marketplace repository in grimoire-rs/e2e-marketplace showcasing the usage; Make sure there is in the end only one PR for any project ready to merge/release; Explicilty allowed to create a github project and maintain the main branch as needed; For shorter feedback cycles deploy a dev version manually via the ocx cli to dev.ocx.sh/grimoire/cli (with timestamp), which can then be used in the e2e env
- Forbidden or narrowed acts: at most one PR per project, and it ends ready to merge/release ("Make sure there is in the end only one PR for any project ready to merge/release.").

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

- Entry point: Run the /hex-plan skill on "Harness marketplace export — `grim export marketplace`, committed marketplace repo, render-derived plugin versions, per .agents/adr/adr_harness_marketplace_export.md".
- Refinement rounds: 2 — counts outer cycles: each review ⇄ execute
  pass is one, and so is every failed repair or retry cycle — a local
  verify failure, an execute or finalize retry, a post-finalize CI fix ⇄
  re-finalize pass. Inner review-fix rounds do not count, and their limit
  is untouched.
  Past `2`, the DONE block reports every remaining criterion `not met`
  and the run stops.
- Inner loop: But ensure to keep inner review/execute loops to an inlined minimum, defering most to a bounded  review/exectute loop in the end.
- Ticks: /hex-loop commits nothing. The session creates or switches to
  the branch the pasted prompt's I9 names and commits this file first on
  it. A box is ticked only between hex-mode runs, never during one, and
  each tick is committed at once; every tick lands before the final
  /hex-finalize, none after it. The `(DONE block only)` criteria are
  evidenced only in the closing DONE block, never ticked.

## Rules

- Run rules:
  - Pull qoder in.
  - Make sure there is a use-case centric documentation explaining the native integration without grim, into cloude/web environment or for customers.

## Emphasis

- Make sure to use more Sonnet than usual.
- Also use the windows host to run tests before running the deep verify in the end.
- Risks carried from the hex-architect run (2026-09-29): the Round-1 re-validation left 1 Block and 12 Warn outstanding in the ADR/design — the planning pass resolves them first:
  - Block: the verification job copies only `marketplace.toml`/`.lock` into a temp dir, so logos, `path:` members and `project` plugins break the check → run `--marketplace $WORKSPACE/marketplace.toml -o $tmp`.
  - Warn: landing page renders Cursor unconditionally; verification compare set undefined; `.qoder-plugin/` / `.factory-plugin/` shadowing unchecked; porcelain `R`/`C` wording; required check behind a `paths:` filter stays Pending; `GITHUB_TOKEN` fallback cannot satisfy the required check; empty-parent pruning rule differs ADR vs design; first-run crash leaves an unowned `./<c>/`; `[marketplace].name` rename behaviour; adding Cursor to defaults after 1.0; `.cursor-plugin/plugin.json` vs byte-equality with `export plugin`; `empty` vs `removed` row overlap; zero-plugin path hits `NoneDeclared` (`src/export/stage.rs:283-287`).
- ADR open questions #1–#3 (stateless v1, GitHub App vs `GITHUB_TOKEN`, release attestations) have no owner answer; each carries a `Recommended:` line. "Pull qoder in." overrides the ADR's Qoder deferral (D2/D12): Qoder gets its own family mapping, `.qoder-plugin/marketplace.json` and `./qoder/<plugin>` trees, with a D11 row and release note for the `export plugin` default-output change.

## Context

- ADR: .agents/adr/adr_harness_marketplace_export.md (Proposed; D1–D13, Open questions, Amendments)
- System design: .agents/specs/design_harness_marketplace_export.md
- Dossier: .agents/discussions/marketplace-phase-2.md
- Phase-1 ADR: .agents/adr/adr_harness_plugin_export.md; spec .agents/specs/design_harness_plugin_export.md
- Research: .agents/research/research_marketplace_manifest_schemas.md, research_derived_version_compat.md, research_marketplace_ci_write_path.md, research_marketplace_multi_harness_root.md, research_marketplace_hosting_prior_art.md, research_marketplace_multi_version.md, research_marketplace_phase2_recon.md, research_incremental_pages_builds.md, research_url_marketplace_trust.md
- Sibling repos: /home/mherwig/dev/grimoire-indexer, /home/mherwig/dev/grimoire-components, /home/mherwig/dev/grimoire-index
- Windows test runner: /mnt/c/Users/ecom/grim-wintest/run.sh
- Deep verify workflow: .github/workflows/verify-deep.yml
