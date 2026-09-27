# Discussion: Recurring harness-capability freshness loop

State: handed-off → loop · Updated: 2026-09-27
Ratified: 2026-09-27 → loop
Confidence: ratified by the owner via /hex-loop invocation after the restate; research vintages 2026-09-27 (five artifacts under ## Research)

## Intent

Keep grim's knowledge of every supported AI-agent harness state of the art:
new things to render for a vendor, newly supported vendor metadata, changed
config paths and env vars, and stale documentation. Sonnet subagents do the
research; the procedure lands as a skill in this repository. A goal loop
builds the skill, runs it domain by domain, and refines the skill after every
run. The vendor research itself is the loop's work, not this discussion's.

Why now: every dated upstream claim is re-verified only reactively, when
someone edits that vendor's renderer
(`.claude/rules/vendor-capability-watchlist.md` "Re-verify procedure"). Nothing
sweeps on a calendar and no workflow runs on a schedule, so rows rot silently
(it already happened: `xhigh` reasoning effort, Codex `additionalContext`).

Out of scope:
- Dependency and toolchain versions — Dependabot's job.
- Scheduled cloud routines or cron workflows.
- Adding a whole new client/vendor — a separate feature; a sweep that finds
  a candidate records it as an issue.
- Class-3 runtime compensation (plugins, wrappers) — never in scope per the
  watchlist.
- Any breaking change (AGENTS.md Principle 9) — findings that need one become
  issues, never code.

## Requirements

- **Skill.** New project skill at `.claude/skills/upstream-refresh/`,
  registered in `.claude/rules.md` "Skills by task topic". Invocation:
  `/upstream-refresh` (normal sweep, stale-first),
  `/upstream-refresh <harness>` (deep pass on one harness),
  `/upstream-refresh --domain <domain>` (one domain). The watchlist rule's
  "Re-verify procedure" points at the skill.
- **Domain registry** in `.claude/skills/upstream-refresh/references/`: one
  entry per freshness domain naming its ledger (the file that holds its dated
  claims), its sources, and its change feed. Domains:
  1. Vendor capabilities, metadata keys, config paths and env vars —
     ledger `.claude/rules/vendor-capability-watchlist.md`, code
     `src/install/vendor_*.rs`, docs `docs/src/content/docs/clients.md`,
     `vendor-metadata.md`, `mcp-servers.md`, and the `AGENTS.md` env-var
     table (currently undated).
  2. Catalog skills — `catalog/skills/*/references/updating.md` protocols.
     These already fire reliably at every grim release
     (`.agents/research/research_repo_reverification_history.md`), so the
     sweep only confirms the protocol ran for the latest release tag and
     never re-researches it.
  3. Specs — MCP spec claims (`docs/src/content/docs/mcp-servers.md`,
     `src/oci/mcp.rs`) and agentskills.io.
  4. Forge API claims — the ratings-forge rows (`ratings.md`,
     `src/catalog/rating_provider.rs`).
  5. Product landscape — `.claude/rules/product-context.md` comparable tools.
  6. Expired research — `.agents/research/*.md` past their `Expires:` date.
- **Check ledger.** `.agents/upstream-checks.md`, committed, one row per
  harness and per domain: last check date, the depth that check reached, and
  a feed cursor (the last changelog version or entry seen). It is separate
  from per-claim `verified` dates: a claim's date says when that fact was
  confirmed, a check row says when anyone last looked. It lives outside the
  skill directory, so a run never counts as a skill change.
- **Depth ladder**, decided per target from its check-ledger row:
  - checked today → no-op, unless `--force`;
  - checked 1–2 days ago → changelog scrape only: read the feed from the
    cursor forward and act on relevant entries, unless the invocation names
    what to look for;
  - 3+ days → normal sweep;
  - Tier 1 deep pass only when its last deep pass is 30+ days old, or the
    harness is named explicitly.
  An explicit focus ("check Codex hooks") always overrides the ladder for
  that focus.
- **Evidence per claim.** Each ledger row carries `verified <date>`, a
  primary-source URL, and the vendor version when one is known (porex-bot's
  `verified_at`/source precedent,
  `.agents/research/research_multi_harness_tool_freshness.md`). Undated
  claims found in a domain get dated on their first sweep.
- **Normal sweep.** Stale-first: rows past the threshold (the watchlist's
  existing ~6 months), plus any domain whose change feed shows relevant
  entries since its oldest `verified` date. One sonnet researcher per domain
  or harness. Feeds per harness:
  `.agents/research/research_harness_change_feeds.md`.
- **Deep pass** — Tier 1 harnesses (`claude`, `codex`, `opencode`, `copilot`,
  per `.agents/adr/adr_vendor_support_tiers.md`) and any harness named at
  invocation:
  - Full surface inventory: list the vendor's entire current config surface
    (artifact kinds, frontmatter keys, hooks, MCP options, config paths, env
    vars) and diff it against grim's renderer, not just the stale rows.
  - Live CLI check where the CLI installs headlessly: render into a sandbox
    and confirm the harness loads it (how existing `live-verified` comments
    were earned).
- **Verification.** Researchers cite a primary-source URL and a quoted
  excerpt for every changed claim; a claim they cannot source is dropped, not
  guessed. An opus verifier re-checks every claim that drives a renderer or
  metadata change before it lands.
- **Routing by risk.** Ledger dates, docs and matrix rows are fixed directly.
  Additive renderer or metadata support is implemented with tests in the same
  run: renderer, docs, parity test and watchlist row in one commit, per the
  watchlist doctrine. Anything touching compatibility or security, and any
  new-vendor candidate, becomes a GitHub issue.
- **Staleness report.** A `task` target (e.g. `task upstream:stale`) lists
  every ledger row past the threshold and every research artifact past
  `Expires:`. It is warn-only and not part of `task verify`, so the calendar
  never turns CI red.
- **Refinement.** Each run ends by recording friction (a wrong feed, a
  researcher that guessed, a domain whose ledger was unclear) and updating the
  skill before the next run.

## Decisions

- Routing: tier by risk.
- Scope: every dated upstream claim, through a domain registry.
- Cadence: manual skill run plus a warn-only staleness report.
- Verification: sonnet research with URL and quote; opus gate before code.
- Home: new skill `.claude/skills/upstream-refresh/`;
  `meta-validate-context` and `meta-maintain-config` keep their
  repo-internal scope.
- Loop: one domain per iteration, refining the skill after each; the deep
  pass for Tier 1 and explicitly named harnesses adds a full surface
  inventory and a live CLI check.
- Depth ladder: same day no-op, 1–2 days feed-only, 3+ days sweep, Tier 1
  deep at 30 days or when named; timestamps in `.agents/upstream-checks.md`.
- Order: (1) build the skill, the staleness task and the check ledger; (2–5) deep pass on
  `claude`, `codex`, `opencode`, `copilot`, one per iteration; (6) normal
  sweep of the Tier 2 vendors; (7+) catalog, specs, forge, product landscape,
  expired research.

## Research

- `.agents/research/research_upstream_claim_surfaces.md` — every upstream
  claim in the repo, its date stamp, and its current owner.
- `.agents/research/research_capability_matrix_freshness.md` — prior art
  (MDN BCD, multi-agent sync tools, LLM fact-checking mitigations).
- `.agents/research/research_multi_harness_tool_freshness.md` — how
  rulesync, ruler, porex-bot and others adopt new vendor features.
- `.agents/research/research_harness_change_feeds.md` — per-harness
  changelog feeds, doc sources, and schemas.
- `.agents/research/research_repo_reverification_history.md` — one
  proactive vendor sweep ever (`1ca82e2`); vendor research never revisited;
  catalog release protocol fires reliably. Seed finding for the Copilot pass:
  re-verify the `${VAR}` MCP substitution row past copilot-cli v0.0.407.

## Related

- `.claude/rules/vendor-capability-watchlist.md` — the existing dated ledger
  and re-verify doctrine.
- `.agents/adr/adr_vendor_support_tiers.md` — Tier 1 set and compensation
  classes.
- `catalog/README.md` — content drift tiers.
- `.claude/skills/meta-validate-context/`, `.claude/skills/meta-maintain-config/`
  — adjacent freshness skills whose scope excludes upstream facts.
- `src/install/client_target.rs` —
  `docs_matrix_row_set_matches_all_and_cells_track_kind_support` and
  `kind_support_grid_matches_adr_mapping_table` parity tests.

## Open questions

- [NEEDS CLARIFICATION: Can the live CLI check run without vendor
  credentials, and in what sandbox?]
  Recommended: use each harness's config-listing or dry-run command in a
  throwaway `$HOME` (`HOME=$(mktemp -d)`), and skip the check with a noted
  reason where the CLI requires login — never use the owner's credentials.
- [NEEDS CLARIFICATION: Does a sweep commit straight to a branch, or open a
  PR per domain?]
  Recommended: one branch per loop iteration with Conventional Commits,
  landed by the owner; no pushes from the loop (AGENTS.md Principle 6).

## Verification

- Iteration 1: `task claude:tests` passes with the new skill registered in
  `.claude/rules.md`; `task upstream:stale` runs and lists today's stale
  rows and expired research; `.agents/upstream-checks.md` exists with a row
  per harness and domain.
- Depth ladder: a second `/upstream-refresh` on the same day is a no-op
  that says so; `--force` overrides it; a run 1–2 days later only reads
  feeds and advances the cursors.
- Every iteration: `task verify` green; the parity tests in
  `src/install/client_target.rs` pass; every changed ledger row carries a
  new `verified` date, a source URL and a quoted excerpt in the iteration's
  research artifact; every code-driving claim has an opus verifier sign-off
  in that artifact; every deferred finding has a GitHub issue link.
- Every iteration records its skill changes (or "no change") in the
  discussion's hand-off record, so refinement is auditable.
- Done when every domain has been swept once, `task upstream:stale` reports
  nothing past the threshold, and the final iteration needed no skill change.

## Hand-off record

- 2026-09-27 · claude · deep · skill: deep-pass MCP fixture via loopback registry, native Claude binary, current `claude mcp list` note, find-delete cleanup; sweep/deep cursor + changelog range; researcher-brief deep slot and fetch tips · artifact: .agents/research/research_upstream_claude_20260927.md
- 2026-09-27 · codex · deep · skill: deep-pass codex row (telemetry `-c` opt-outs, prompt-input skill check) and note (benign /tmp warning, trusted-project entry, agents skip, no doctor); researcher-brief first-pass cursor n/a · artifact: .agents/research/research_upstream_codex_20260927.md
- 2026-09-27 · opencode · deep · skill: deep-pass opencode row (native binary, TMPDIR, autoupdate/models-fetch opt-outs, debug skill/config verbs) and note (no login, both scopes, grim on PATH for mcp list, strict config validation, zsh word-splitting); domains.md live-check cell · artifact: .agents/research/research_upstream_opencode_20260927.md
- 2026-09-27 · copilot · deep · skill: deep-pass copilot row (native binary, COPILOT_OFFLINE/COPILOT_AUTO_UPDATE opt-outs, mcp get, --agent probe) and note (node-less loader, project MCP never lists, dead-BYOK-port recipe for expansion); domains.md live-check cell; resolved seeds replaced · artifact: .agents/research/research_upstream_copilot_20260927.md
- 2026-09-27 · cursor, kiro, junie, gemini, zed, amp, antigravity, cline, droid, goose, warp, openclaw, kilo, qoder · sweep (WP-G, `--domain vendors`; claude, opencode, copilot, codex noop) · skill: researcher-brief scratchpad output file + no-curl fallback + verbatim-quote warning; domains.md docs/feed URLs (kiro, junie, amp, antigravity, goose, openclaw, qoder), `JUNIE_HOME`/`OPENCLAW_HOME` cells, section-default and stamp re-dating note; SKILL.md one artifact per target in a `--domain` run · artifact: .agents/research/research_upstream_<target>_20260927.md (14 files)
- 2026-09-27 · catalog · sweep (WP-H, `--domain catalog`) · skill: no change · artifact: .agents/research/research_upstream_catalog_20260927.md
- 2026-09-27 · specs · sweep (WP-I, `--domain specs`) · skill: domains.md §3 ledger/code list (agentskills field table, `src/skill/` limits, all MCP spec links, client-implementation guide, repo-at-tag spec reads); researcher-brief link-liveness fallback · artifact: .agents/research/research_upstream_specs_20260927.md
- 2026-09-27 · owner feedback · — · skill: default run simplified — every stale target (Tier 1, Tier 2, domains) checked at once by one sonnet subagent each, no tier ordering; ledger/doc date fixes land directly; code-driving findings reported, and one batched opus verify + one task verify only when code lands; deep pass only on a named harness; unfinished targets stay stale · artifact: —
