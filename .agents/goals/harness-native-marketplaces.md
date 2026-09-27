# Goal: "Harness-native marketplaces rendered from the catalog"

Source: .agents/discussions/harness-native-marketplaces.md · Written: 2026-09-27 by /hex-loop

## Definition of done

- [x] "An ADR fixes the `marketplace.toml`/`marketplace.lock` schema, export JSON output and exit codes before any code." — evidence: .agents/adr/adr_harness_plugin_export.md, landed in 69e2c25d, ahead of every code commit in the series (Agent Plugins `mcp.json` reinstated during planning).
- [x] "`grim export plugin` exports ad-hoc refs and declared plugins per `--client`, as a folder or `--zip`." — evidence: `grim export plugin` in 7459d1cc; test/tests/test_export_plugin.py S-001/S-002/S-003/S-005/S-010, unit c014_*/c015_*; /hex-review Approve (L3, plan State done).
- [x] "Export output is rendered exactly like `grim install` and is byte-reproducible across runs." — evidence: test_s011_* byte-compares against a real `grim install` for claude, codex, copilot, junie, openclaw (7459d1cc); test_s012 varies TZ/umask/cwd/out-dir; c030_golden_zip_sha256 pins the zip digest. One strict xfail (S-007 offline warm cache, plan decision 36: no manifest cache, install parity).
- [x] "Claude and Agent Plugins 1.0 manifests carry the plugin name, derived version and named omissions." — evidence: src/export/family.rs (7459d1cc); test_s001, test_s013, test_s019, test_s028, test_s031 (Agent Plugins `mcp.json`).
- [x] "`grim update --marketplace` rolls selected plugin pins forward without installing anything." — evidence: 7459d1cc; test/tests/test_update_marketplace.py test_s020 (workspace and HOME trees unchanged), test_s021–s023; `run_marketplace` never reaches installer, prune or install state (review-orch trace).
- [x] "`[plugins.x.rename] strip_prefix` renames members and fails closed on empty names, collisions and stale references." — evidence: src/export/rename.rs (7459d1cc); test_s014–s016, unit c021_*/c022_* including `_`-adjacent stale-name cases.
- [x] "No duplicate lock, update or render path: every new behaviour reuses the seams in Decisions › Reuse mandate." — evidence: seams extracted in cf06fc60; `MarketplaceLock` = map of `GrimoireLock` on shared `RawLock`/`lock_io` (7459d1cc); one `roll_forward` moved to src/resolve/resolver.rs (7459d1cc); render via `stage_locked_artifact` + `ClientTarget::materialize`; `grim schema --kind lock` golden unchanged; review-orch traced every call site and removed three small duplications.
- [x] "Acceptance tests, docs pages and the `grim-usage` catalog drift review land with the feature." — evidence: test_export_plugin.py, test_update_marketplace.py, test_export_update_flow.py (1248 acceptance passed + 1 xfail); docs commands/configuration/stability/json-interface/guides team-plugin and grim-usage + grim-authoring drift review (2ea5ef0d, 2ea5ef0d); `task docs:check` and `task catalog:verify` green.
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

- Entry point: Run the /hex-plan skill on "Harness-native marketplaces rendered from the catalog, per .agents/discussions/harness-native-marketplaces.md".
- Refinement rounds: 2 — counts outer cycles: each review ⇄ execute
  pass is one, and so is every failed repair or retry cycle — a local
  verify failure, an execute or finalize retry, a post-finalize CI fix ⇄
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

- Source: .agents/discussions/harness-native-marketplaces.md
- Research: .agents/research/research_plugin_support_matrix.md, .agents/research/research_plugin_format_compat_a.md, .agents/research/research_plugin_format_compat_b.md, .agents/research/research_plugin_format_compat_c.md, .agents/research/research_agent_plugins_spec_verify.md, .agents/research/research_claude_app_install_surfaces.md
- Swarm memory: .agents/memory/hex.md
