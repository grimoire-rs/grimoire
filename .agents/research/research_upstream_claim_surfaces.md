# Research: Upstream-claim freshness surfaces in this repo

## Metadata

**Date:** 2026-09-27
**Domain:** cli
**Triggered by:** `.agents/discussions/harness-capability-freshness.md` — entry recon (codebase lane)
**Expires:** 2027-03-27

## Direct Answer

Every fact grim encodes about an external upstream, and who re-verifies it
today. Only vendor-capability rows carry `verified <date>` stamps, and their
re-verification is reactive (path-triggered on vendor renderer edits). No
calendar sweep, no scheduled workflow, and no mechanical staleness check exists
anywhere; the two general freshness skills explicitly exclude vendor facts.

## Surfaces

| Surface | Upstream fact | Dated? | Owner today |
|---|---|---|---|
| `.claude/rules/vendor-capability-watchlist.md` (watchlist, wave-1/2, Qoder, ratings-forge tables) | per-vendor decline/skip/warn reasons, env overrides, MCP transport/oauth, forge versions | yes, oldest `2026-07-17`, newest `2026-09-24` | the rule itself, reactive on edit |
| `src/install/vendor_*.rs` module docs | config-path shape, native root, live-verified rendering | yes, inline `verified`/`live-verified` dates | watchlist rule |
| `src/install/vendor_claude.rs:8` | Claude frontmatter registry | "verified", no date | watchlist rule |
| `src/install/client_target.rs` `kind_support` grid | vendor × kind support | no — mechanical | parity tests |
| `docs/src/content/docs/mcp-servers.md:116,449` | SSE deprecated in MCP spec | no | nobody (reactive via `src/oci/mcp.rs`) |
| `docs/src/content/docs/clients.md` | "every ◐/✗ traces to a verified upstream limitation" | dates live in watchlist | watchlist + parity test |
| `docs/src/content/docs/vendor-metadata.md`, `ratings.md` | vendor metadata keys; GHES/GitLab version claims | dates in watchlist | watchlist rule |
| `AGENTS.md` env-var table | vendor config-dir overrides (`CLAUDE_CONFIG_DIR`, `COPILOT_HOME`, `CODEX_HOME`, `KIRO_HOME`, `QODER_CONFIG_DIR`, `GEMINI_CLI_HOME`, `OPENCODE_*`) | no | nobody |
| `catalog/skills/*/references/updating.md` | agentskills.io authority chain, vendor field registries, caps | no — "on every grim minor release" | package maintainer, manual |
| `.claude/rules/product-context.md` comparable tools | competitive landscape | one date: re-verify after `2027-01-26` | nobody enforces |
| `.agents/research/research_vendor_verification_*.md` | sources behind watchlist rows | per-claim confidence tags, no uniform date | feeds watchlist |
| `.claude/rules/product-tech-strategy.md` | "latest stable" versions | no | nobody |

## Existing procedures and gaps

- Watchlist re-verify: reactive, `> ~6 months` rule is prose only — a row
  rots indefinitely if nobody edits that vendor.
- Catalog drift review: reactive on docs/`src/command/**` edits;
  `task catalog:verify` checks schema only, not facts.
- `updating.md` refresh: manual per minor release, no automation.
- `meta-validate-context` (subsystem rules vs `src/`) and
  `meta-maintain-config` (`.claude/` quality): monthly, both exclude
  vendor/upstream facts.
- `.github/workflows/`: no `schedule:` trigger anywhere.

## Mechanical guards

- `kind_support_grid_matches_adr_mapping_table` (`src/install/client_target.rs:626`).
- `docs_matrix_row_set_matches_all_and_cells_track_kind_support`
  (`src/install/client_target.rs:748`) — parses `clients.md` at test time.
- `.claude/tests/test_ai_config.py` catalog-parity tests (config, not vendor).

## Negative

No mechanical enforcement of the 6-month rule; no MCP spec-version literal in
`src/oci/mcp.rs`; no re-verify procedure for tool versions.

## Leads

- Check whether `updating.md` refreshes ever actually fired (git log).
- Check `research_vendor_verification_*.md` dates against watchlist rows.
- Check hook reminders for `AGENTS.md` env-table edits.
