# Research: Repo history of upstream re-verification

## Metadata

**Date:** 2026-09-27
**Domain:** cli
**Triggered by:** `.agents/discussions/harness-capability-freshness.md` — repo archaeology lane
**Expires:** 2027-03-27

## Direct Answer

Vendor facts were re-verified proactively exactly once. Every other watchlist
edit rode along with a renderer change or vendor onboarding, and each vendor
research artifact was written once and never revisited. The catalog skills'
release-tied `updating.md` protocol, by contrast, fires reliably on release
day with real content diffs.

## Findings

- **Watchlist** (`.claude/rules/vendor-capability-watchlist.md`, 16 commits,
  `fe21b60` 2026-07-17 → `400b734` 2026-09-24): all but one commit pair a row
  edit with renderer or onboarding work (`e54b542`, `f1b78d4`, `ac4c1dc`,
  `400b734`, support-tier commits `691395b`, `7dce918`). The single proactive
  sweep is `1ca82e2` (2026-07-17): it corrected the Copilot `${VAR}` MCP
  substitution citation (shipped in copilot-cli v0.0.406, regressed in
  v0.0.407, `github/copilot-cli#1403`) and a Codex oauth claim. Wave-0 rows
  (Codex, Copilot, OpenCode) still carry `verified 2026-07-17`.
- **Vendor research artifacts** (`research_vendor_verification_{cursor_kiro,
  junie_gemini,qoder,wave2_batch,zed_amp}.md`): one commit each, in the
  onboarding commit (`b58c61d`, `ac4c1dc`, `400b734`); never re-verified.
- **Incident** `6d93276` (2026-07-17): restored Codex `xhigh` after
  `3328de5a` dropped it on a refuted premise, alongside a stale Codex
  `PreToolUse`/`additionalContext` claim (superseded by `openai/codex#20692`).
  Caught within about an hour, same session — the two "it happened" examples
  the watchlist cites.
- **Catalog `updating.md`**: `grim-usage` `compatibility` bumped the same day
  as each release — 0.9, 0.11 (0.10 skipped), 0.12, 0.13 (`02cc9344`), 0.14
  (`c74a9aaf`) — with real content diffs (`557fb57b`). The release trigger
  works.
- **Related parked work**: `.agents/adr/adr_catalog_freshness_revalidation.md`
  (`29ded5a`, Proposed, parked) covers registry/TUI cache freshness, not
  vendor facts.

## Negative

No drift test compares the compat matrix against live upstream docs; the
parity test (`src/install/client_target.rs:759`) checks internal consistency
only.

## Leads

- Re-verify the Copilot `${VAR}` row against a copilot-cli release past
  v0.0.407.
- Check whether `github/copilot-cli#1403` and `grimoire#44` closed upstream.
