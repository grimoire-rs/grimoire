State: handed-off → loop · Updated: 2026-09-28
Ratified: 2026-09-28 → loop
Confidence: ratified by owner (explicit yes); research vintages 2026-09-28 (`research_hooks_pr98_drift.md`, `research_hooks_upstream_2026-09.md`)

# Revive PR #98 (hooks artifact kind) for release

## Intent

PR [grimoire-rs/grimoire#98](https://github.com/grimoire-rs/grimoire/pull/98) (`hex/hooks-artifact-kind`, 11 commits, +70k/-750, 251 files; merge base `d6cecb85`, 2026-08-28) adds `ArtifactKind::Hook` behind `[options.experimental] hooks`. Main moved 90 commits / 732 files since; the PR is `CONFLICTING` (36 files), its green CI is stale, and it has no human review. Goal: bring it current with main, conform it to everything main added since (rules, Starlight docs, install seams, new clients, plugin export), fold in the upstream facts that changed, and make it mergeable so it ships in the next release as an experimental, off-by-default feature.

## Requirements

- Squash the branch to one commit, then rebase once onto `origin/main` (one port pass instead of 11); PR #98 stays the vehicle; `/finalize` re-splits into Conventional Commits before merge.
- Re-plumb hook install through main's extracted seams (`92699721`: declare, client-list, staging, rebind, roll-forward in `src/command/add.rs`, `src/command/update.rs`, `src/install/installer.rs`, `src/install/target.rs`) and reconcile with main's vendor rewrites (`src/install/vendor.rs`, `vendor_claude.rs`, `vendor_codex.rs`, new `vendor_qoder.rs`), `json_splice.rs` escaping (`2043a627`), `.grimignore` packing/hashing (`d46709d7`).
- Wire Qoder hooks (Claude-shaped upstream, no documented trust model — grim's flag + workspace consent remain the only gate) and Copilot mutator (`modifiedArgs`).
- Correct ADR facts in `.agents/adr/adr_hooks_support.md` / `adr_hook_workspace_consent.md` (Gemini `trustedFolders.json`; OpenCode plugin-API-only; Copilot `modifiedArgs`, `SessionStart.additionalContext`, cloud-agent hook support) and record the export decline + Qoder/Copilot additions as amendments; add hook rows to `.claude/rules/vendor-capability-watchlist.md`.
- `grim export plugin` declines hook members with a warning + README listing (exit code unchanged, JSON output additive only).
- Docs on Starlight: one hooks use-case page under `docs/src/content/docs/` with `doc_type`/`doc_tier` and an experimental marker; hook entries in `artifacts.md`, `commands.md`, `json-interface.md`, `clients.md`, `configuration.md`; drop the PR's `docs/src/hooks.md` and `docs/src/SUMMARY.md` edits; register the page in `.agents/discovery/use-cases.yaml`.
- Obey rules added since the base: Principle 9 additive-only (hook-free projects byte-identical), `subsystem-config-keys.md` for the new config key, catalog drift duty (`catalog/README.md`: `grim-usage`, `grim-authoring`, `ai-config-authoring`), `docs-quality.md` declarations.

## Decisions

- Bring PR #98 current by **rebasing** `hex/hooks-artifact-kind` onto `main` (owner's choice over re-cut/port; the PR and its history stay).
- Land as **one PR** (#98), not split.
- Release hooks **experimental, behind `[options.experimental] hooks`, off by default** — as the PR designed; stable surfaces stay untouched (Principle 9).
- Upstream catch-up in this PR: correct ADR facts (Gemini `trustedFolders.json`, OpenCode plugin-API-only, Copilot `modifiedArgs` + `SessionStart.additionalContext`, Copilot cloud agent), **wire Qoder hooks** and **Copilot mutator support**, add hook rows to `.claude/rules/vendor-capability-watchlist.md`.
- Docs: one hooks **use-case page** (with `doc_type`/`doc_tier`, marked experimental) under `docs/src/content/docs/`, plus hook entries in the reference pages (artifacts, commands, json-interface, clients); PR's old `docs/src/hooks.md` and `SUMMARY.md` edits dropped.
- `grim export plugin` (`src/command/export.rs`) **declines hook members with a warning** and lists them in the exported plugin's README. Reason: exported plugins run without grim, and a hook's contract (`GRIM_HOOK_*` env, canonical stdin envelope, tier ordering, timeouts, verdict projection) is grim's runtime. A grim-free hook runtime (TS/Python SDK vs generated shim) is a **follow-up issue + `/hex-architect`**, not this PR.


## Research

- `.agents/research/research_hooks_pr98_drift.md` — drift map of PR #98 vs main: docs layout obsolete (mdBook → Starlight, no `doc_type`/`doc_tier`), install seams refactored (`92699721`), parallel `vendor.rs` rewrites, export-plugin has no hook story; CI green but stale, no human review comments.
- `.agents/research/research_hooks_upstream_2026-09.md` — upstream hook systems vs the PR's ADR claims: no client lost support, core design assumptions hold. Corrections: Gemini trust store is `trustedFolders.json` (ADRs say `trusted_hooks.json`); OpenCode has no native hooks file (JS/TS plugin API only); Copilot now documents `modifiedArgs` (mutator) and `SessionStart.additionalContext`, and its cloud agent supports hooks; Cursor out of beta, allow/ask-not-enforced bug still open. Qoder (added on main) has a Claude-shaped hook system the PR never surveyed.

## Related

- PR [grimoire-rs/grimoire#98](https://github.com/grimoire-rs/grimoire/pull/98); on its branch: `.agents/adr/adr_hooks_support.md`, `.agents/adr/adr_hook_workspace_consent.md`, `.agents/plans/plan_hooks_artifact_kind.md`.
- On main: `.agents/adr/adr_harness_plugin_export.md`, `.agents/adr/adr_render_layout_stability.md`, `.agents/plans/plan_docs_site_redesign.md`, `docs/src/content/docs/stability.md`.

## Out of scope

- Grim-free hook runtime / author SDK (TS, Python) — follow-up issue + ADR.
- Hooks in exported plugins; graduating hooks out of experimental; project scope beyond Claude (ADR A1 stands); Copilot cloud agent (ADR D7 stands).
- Splitting the PR; re-cutting from main.

## Open questions

- [NEEDS CLARIFICATION: Qoder hook scope — global-only like Codex/Copilot, or project too?]
  Recommended: global-only — ADR A1 limits project scope to clients with a non-environment-derived launcher path; Qoder was never assessed against it.
- [NEEDS CLARIFICATION: Copilot cloud agent now supports hooks; keep D7's exclusion?]
  Recommended: keep excluded, note in watchlist — no local consent surface in a cloud agent.

## Verification

- `git merge-tree --write-tree origin/main <branch>` clean; PR #98 `mergeable: MERGEABLE`.
- `task verify` and `task docs:check` pass; `task catalog:verify` passes.
- Windows acceptance via `/mnt/c/Users/ecom/grim-wintest/run.sh`, compared against `origin/main`.
- Principle 9: hook-free fixture projects produce byte-identical files and hashes vs main; flag off → no dispatcher registration anywhere.
- Acceptance tests: Qoder + Copilot mutator hook install/run; `grim export plugin` with a hook member warns, omits it, lists it in README, exits 0.
- Fresh CI green on the rebased head; `/hex-review` approves before `/finalize`.
