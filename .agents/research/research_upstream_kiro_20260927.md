# Research: upstream refresh — kiro (sweep)

## Metadata
**Date:** 2026-09-27 · **Domain:** vendors · **Depth:** `kiro sweep never checked`
**Triggered by:** /upstream-refresh --domain vendors (WP-G, plan_harness_capability_freshness)
**Expires:** 2027-03-27

Ladder output for the whole `--domain vendors` run (the four Tier 1 rows
were deep-checked earlier today, so `noop`; the 14 Tier 2 rows `sweep`):

```text
cursor sweep never checked
kiro sweep never checked
junie sweep never checked
gemini sweep never checked
zed sweep never checked
amp sweep never checked
antigravity sweep never checked
cline sweep never checked
droid sweep never checked
goose sweep never checked
warp sweep never checked
openclaw sweep never checked
kilo sweep never checked
qoder sweep never checked
claude noop checked today
opencode noop checked today
copilot noop checked today
codex noop checked today
```

Vendor version at check: Kiro CLI **2.24.0** (2026-09-23), IDE **1.1.70**
(2026-09-24), per <https://kiro.dev/changelog/>. Page feed; cursor was `—`,
newest heading **IDE 1.1.70** recorded. Changelog swept 2026-07-17 →
2026-09-27. Relevant entries: `AGENTS.md` loads as steering (CLI 2.18.0, IDE
1.0.309), a global hooks dir `~/.kiro/hooks/` (CLI 2.13.0), "Powers" (IDE
1.0.288) and Kiro Crew 0.7.0 skill search. The docs moved from
`/docs/cli/chat/configuration` to `/docs/configuration/`.

## Claims

Researcher (sonnet) table, verbatim. Numbered rows re-verify ledger claims;
`I` rows are changelog or surface differences found in the sweep.

| # | Claim | Ledger location | Old value | New value | Source URL | Verbatim quote | Vendor version |
|---|---|---|---|---|---|---|---|
| 1 | Agents declined — CLI/IDE `.kiro/agents/` format collision | `.claude/rules/vendor-capability-watchlist.md` L125; `src/install/vendor_kiro.rs` module doc; `docs/src/content/docs/clients.md` gap-kiro-agents | "re-verified 2026-07-26 — still open" (kiro #8040) | unchanged — still open, last updated 2026-09-24 | https://github.com/kirodotdev/Kiro/issues/8040 | "Kiro CLI expects .json files with the full agent config as a JSON object... Kiro IDE expects .md files with YAML frontmatter... Because they look for different extensions in the same directory, you effectively need to maintain two copies of every agent." | issue open as of Kiro CLI 2.24.0 (current) |
| 2 | Global rule `fileMatch` scoping inert until #9176 closes | `.claude/rules/vendor-capability-watchlist.md` L126; `src/install/vendor_kiro.rs`; `docs/src/content/docs/clients.md` gap-kiro-rules | "per-file fileMatch scoping open (#9176)" | GH issue state is now **closed** (2026-08-06), but by the stale-issue bot after inactivity, not a reporter-confirmed fix — a maintainer only said global fileMatch "received several improvements" and asked for a retest that never came | https://github.com/kirodotdev/Kiro/issues/9176 | "This issue has been automatically closed due to inactivity. It has been 7 days since we requested additional information." (bot, 2026-08-06); maintainer, 2026-07-29: "Could you retest on the latest version (IDE 1.0.242) and let us know whether fileMatch from ~/.kiro/steering/ now triggers correctly" | IDE 1.0.242 was the retest ask; no confirmation logged since |
| I1 | Kiro CLI does not support inclusion modes at all (broader than the #9176 global-only framing) | Same location as #2 — `docs/src/content/docs/clients.md` gap-kiro-rules describes the gap as "global-scope scoped rule... ignored... until #9176", but current vendor docs describe a CLI-wide limitation, not global-only | not documented anywhere in the ledger | Kiro's own docs now state inclusion modes (`always`/`manual`/`auto`/`fileMatch`) do nothing on the CLI at **any** scope — every steering file in `.kiro/steering/` loads unconditionally, regardless of the `inclusion`/`fileMatchPattern` frontmatter grim writes | https://kiro.dev/docs/steering/ | "On Kiro CLI, inclusion modes are not currently supported. All steering files in the `.kiro/steering/` directory are loaded automatically." | docs page last updated 2026-09-25 (no CLI version stated) |
| 3 | `KIRO_HOME` honored by the CLI, replaces `~/.kiro` outright | `.claude/rules/vendor-capability-watchlist.md` L128; `src/install/vendor_kiro.rs` module doc; `docs/src/content/docs/clients.md` L405; `docs/src/content/docs/vendor-metadata.md` L461 | "verified 2026-07-26" against https://kiro.dev/docs/cli/chat/configuration: "Agents, prompts, skills, steering, settings, and sessions all resolve against KIRO_HOME" | Doc still confirms the mechanism, but (a) the source URL now 301s to `/docs/configuration/` and the current wording drops "prompts" from the list, and (b) multiple maintainer-acknowledged **open** bugs show `KIRO_HOME` is not reliably honored in practice: v3 interactive sessions ignore it entirely, and skill/agent/steering discovery inconsistently falls back to `~/.kiro` | https://kiro.dev/docs/configuration/ ; https://github.com/kirodotdev/Kiro/issues/11348 ; https://github.com/kirodotdev/Kiro/issues/10417 | "Agents, skills, steering, settings, and sessions all resolve against `KIRO_HOME` when it's set" (docs, updated 2026-09-02); "kiro-cli not consistently resolving skills, agents, steering, hooks, and settings from `KIRO_HOME` during a running session is a real config path regression" (maintainer AnilMaktala, 2026-09-23, #11348) | bug reports filed against Kiro CLI 2.13.0–2.21.4; #11348 still open against current 2.24.0 |
| 4 | Kiro IDE still hardcodes `~/.kiro`, ignores `KIRO_HOME` | `.claude/rules/vendor-capability-watchlist.md` L128; `src/install/vendor_kiro.rs` | "closed by bot mis-triage as dup of #6401, unrelated; gap confirmed open via changelog absence, not issue state" (kiro #9148) | unchanged — #9148 still closed the same way (dup-closed 2026-06-08, reporter objected 2026-07-18 that the real ask was never addressed); no IDE changelog entry from 2026-07-17–2026-09-27 grants IDE `KIRO_HOME` support | https://github.com/kirodotdev/Kiro/issues/9148 | "I believe this is incorrect! You did not address the main ask: The IDE should respect KIRO_HOME (as the CLI does)!" (herrwieger, 2026-07-18, on the bot's dup-closure) | IDE 1.1.70 current; no fix shipped |
| 5 | MCP docs added `disabledTools` + remote `oauth`/`oauthScopes` (not yet emitted by grim) | `.claude/rules/vendor-capability-watchlist.md` L127 | "docs added disabledTools + remote oauth/oauthScopes" → "projection candidates" | unchanged — still documented, still not emitted by grim (oauth descriptors are skipped per `src/install/vendor_kiro.rs`) | https://kiro.dev/docs/mcp/configuration/ | "To keep a server active but prevent an agent from using specific tools, use `disabledTools`"; "`oauth` - Object - No - OAuth configuration for servers that require authentication" | docs page last updated 2026-09-25 |

Unsourced:
- "grim follows the CLI because grim is a CLI tool" reasoning being still valid product-wise (i.e. whether AWS still ships Kiro as CLI+IDE with the CLI as grim's intended target) — no primary source states grim's own design intent; this is grim's own doc, not a vendor claim, so left out of the table.
- Whether the `fileMatch`-inert-on-CLI info box (row I1) is new since the ledger's 2026-07-26 verification or was always true and simply never checked — kiro.dev doesn't version its docs pages, and no changelog entry explicitly announces "inclusion modes now/still unsupported on CLI"; could not date the transition, only confirm current state.

Feed notes:
- Newest tag or heading seen: **Sep 24, 2026 — IDE 1.1.70** ("Configurable Terminal Timeouts, Clickable File Links, and Agent Focus Session Restore"), same-day **Crew 0.7.0**. Cursor found: n/a — cursor was `—`.
- Vendor version current today: **Kiro CLI 2.24.0** (2026-09-23), **Kiro IDE 1.1.70** (2026-09-24) — both per https://kiro.dev/changelog/.
- Anything grim renders/documents that the feed shows changed but no ledger row covers:
  - CLI 2.18.0 (2026-08-12) and IDE 1.0.309 (2026-08-13): `AGENTS.md` files (including nested, anywhere in the workspace tree) now load as steering context — a second steering source grim's rule-authoring docs don't mention. https://kiro.dev/changelog/page/4/
  - Global hooks directory `~/.kiro/hooks/` shipped for CLI (2026-07-17, CLI 2.13.0) and IDE (2026-07-20, IDE 1.0.182) — grim has no `Hook` kind for Kiro at all, so this is currently out of scope, not a gap, but worth noting if hooks are ever modeled. https://kiro.dev/changelog/page/5/
  - "Powers" (an agent-plugin-shaped artifact type, first appearing IDE 1.0.288, 2026-08-07, then synced via Cloud Config from 2026-09-01 onward) is a new Kiro artifact category with its own directory/sync surface that grim does not model at all — no ledger row exists for it. https://kiro.dev/changelog/page/2/
  - Crew 0.7.0 (2026-09-24): skills discovery changed to `skill_search` with `skills.max_triggered` defaulting to `0` instead of auto-injecting every match — unclear whether "Crew" (`kirocrew`, a separate orchestration product under its own `~/.kiro/crew/` subtree) is in scope for grim's `kiro` vendor target at all; flagged, not turned into a row, since it's a distinct binary/product from the CLI/IDE the ledger describes. https://kiro.dev/changelog/page/2/

## Verifier sign-off

One opus verifier (C-008) re-fetched each claim that drives a docs, comment,
watchlist-wording or issue-draft change. No claim in this pass drives a
renderer, metadata or validation change unless Routing says so.

| Claim | Verdict | Evidence URL | Verbatim quote / note |
|---|---|---|---|
| V2 kiro CLI steering | CONFIRMED | https://kiro.dev/docs/steering/ | "On Kiro CLI, inclusion modes are not currently supported. All steering files in the `.kiro/steering/` directory are loaded automatically." |
| V3 kiro#9176 | CONFIRMED | https://github.com/kirodotdev/Kiro/issues/9176 | AnilMaktala (2026-07-29): "Could you retest on the latest version (**IDE 1.0.242**)…" github-actions[bot], 2026-08-06T00:24:24Z: "This issue has been automatically closed due to inactivity." No fix confirmed. Note: the API state_reason is `completed` despite the inactivity close. |
| V4 kiro KIRO_HOME | CONFIRMED | https://kiro.dev/docs/configuration/ ; https://github.com/kirodotdev/Kiro/issues/11348 | Docs: "Agents, skills, steering, settings, and sessions all resolve against `KIRO_HOME` when it's set". /docs/cli/chat/configuration redirects to /docs/configuration/. #11348 is open. AnilMaktala [CONTRIBUTOR], 2026-09-23: "…is a real config path regression — artifact discovery should use `KIRO_HOME` as the root across all subsystems." |
| V5 kiro#8040 | PARTIAL | https://github.com/kirodotdev/Kiro/issues/8040 | Still open, but the issue covers the user-level directory `~/.kiro/agents/`, not project `.kiro/agents/`: "The CLI and IDE expect different file formats for custom agents, but both read from the same directory (`~/.kiro/agents/`)." |

## Live CLI check

n/a — `sweep` depth on a Tier 2 harness (the live check is `deep` only).

## Routing

| Claim | Class | Landed as |
|---|---|---|
| I1 / V2 CLI ignores inclusion modes | (a) + (c) | `clients.md#gap-kiro-rules` rewritten (CLI always-on at every scope); watchlist row; issue draft 1 |
| 2 / V3 #9176 closed by a bot | (a) | watchlist row: the close is not a fix, the warning stays; no render change |
| 3 / V4 `KIRO_HOME` | (a) | new `## Config roots and env vars` row with the open regression [kirodotdev/Kiro#11348](https://github.com/kirodotdev/Kiro/issues/11348) |
| 1 / V5 #8040 | (a) | watchlist row: the collision is in user-level `~/.kiro/agents/` |
| 4 IDE ignores `KIRO_HOME` | (a) | watchlist row dated |
| 5 MCP `disabledTools` / `oauth` | (a) + (c) | row dated; stale skip warning text folded into issue draft 1 |
| feed: AGENTS.md as steering, hooks dir, Powers, Crew | finding | no grim surface; hooks out of scope (PR #98); Crew is a separate product, not a new-vendor candidate for now |
| catalog drift | (c) | issue draft 2 — `catalog/**` is a publish surface, not edited in a pass |

Security: none found.

## Issue drafts

Filed as https://github.com/grimoire-rs/grimoire/issues/150.

### 1. Kiro: the CLI loads every steering file, so scoped rules are always-on for CLI users

**Labels:** `vendor:kiro`, `needs-design`

Kiro's steering docs now say: "On Kiro CLI, inclusion modes are not
currently supported. All steering files in the `.kiro/steering/` directory
are loaded automatically" (<https://kiro.dev/docs/steering/>, verified
2026-09-27, CLI 2.24.0). grim renders a scoped rule as `inclusion: fileMatch`
at both scopes and warns only at global scope (IDE bug
[kirodotdev/Kiro#9176](https://github.com/kirodotdev/Kiro/issues/9176), which
an inactivity bot closed on 2026-08-06 without a confirmed fix). The docs
page now discloses the CLI gap. Decide:

- whether to add a render-layer warning at project scope as well — additive,
  but it changes `warnings` output for every scoped Kiro rule;
- or to leave it at the docs disclosure.

Related: the MCP skip warning says "mcp.json has no oauth surface", but Kiro
now documents a remote `oauth` object and `disabledTools`
(<https://kiro.dev/docs/mcp/configuration/>). The skip is still right while
the shapes differ; the message text is stale.

### 2. Catalog: first-party skills lag the 2026-09-27 Tier 2 sweep

**Labels:** `docs`, `catalog`

`catalog/skills/ai-config-authoring/references/rule-design.md` says Kiro
"honors `fileMatch` steering at project scope". That holds for the IDE only;
the Kiro CLI ignores inclusion modes. `catalog/skills/ai-config-authoring/SKILL.md`
links Goose at `https://block.github.io/goose`, which now redirects to
`https://goose-docs.ai`. `catalog/skills/grim-authoring/references/mcp-spec.md`
lists `timeout` for Claude and OpenCode only, but grim projects it for
Gemini, Qoder and Codex too (already drafted by the codex pass). Fix all
three in a catalog release; the docs site is current.

## Friction

- The domains.md docs URL `kiro.dev/docs/cli/chat/configuration` now redirects to `/docs/configuration/`. Fixed in domains.md.
- V5 corrected the researcher: [kirodotdev/Kiro#8040](https://github.com/kirodotdev/Kiro/issues/8040) is about user-level `~/.kiro/agents/`, not project `.kiro/agents/`.
- Researcher sandboxes denied network `curl` (and, for some, `gh`); the brief's "`curl -sL <url>` is fine" tip was wrong there. Researchers fell back to WebFetch, whose text is model-extracted, so quotes from vendor sites are summarizer-sourced; GitHub sources were read raw via `gh api` where allowed. Brief fixed.
- Driver cost: the brief told researchers to return only the table, so a 14-target run would have re-typed every table. This run gave each researcher one scratchpad output file and assembled the artifacts from it. Brief now names that file.

## Skill changes

One `chore(skills):` commit for the whole WP-G run, [f9effcc1](https://github.com/grimoire-rs/grimoire/commit/f9effcc1): researcher-brief scratchpad output file, no-curl fallback and re-fetch warning; domains.md docs and feed URLs (kiro, junie, amp, antigravity, goose, openclaw, qoder), `JUNIE_HOME` / `OPENCLAW_HOME` cells, section-default and stamp re-dating note; SKILL.md one artifact per target in a `--domain` run.
