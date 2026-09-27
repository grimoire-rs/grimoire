# Research: upstream refresh — junie (sweep)

## Metadata
**Date:** 2026-09-27 · **Domain:** vendors · **Depth:** `junie sweep never checked`
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

Vendor version at check: stable **26.9.22 (3419.7)** per
<https://junie.jetbrains.com/whats-new>; the docs are the 2026-09-25
snapshot. Feed: GH `JetBrains/junie` releases; cursor was `—`, newest tag
**3531.1** (2026-09-25, a nightly with no body) recorded. Release bodies
2026-07-17 → 2026-09-27 read whole: only 2548.5 touches a grim surface (a
project trust gate before loading MCP servers, skills and project config).

## Claims

Researcher (sonnet) table, verbatim. Numbered rows re-verify ledger claims;
`I` rows are changelog or surface differences found in the sweep.

| # | Claim | Ledger location | Old value | New value | Source URL | Verbatim quote | Vendor version |
|---|---|---|---|---|---|---|---|
| 1 | Agent kind (subagents) EAP status | `.claude/rules/vendor-capability-watchlist.md` L129; `src/install/vendor_junie.rs` module doc; `docs/src/content/docs/clients.md` L232-233 | declined — "still EAP" as of 2026-07-26 doc snapshot | **changed — core subagent feature (creation, frontmatter, file locations) is documented as a standing feature with no EAP gate; only the `/settings → Subagents` model-selection-policy setting remains explicitly EAP-gated.** The doc's "Creating a custom subagent" / "File location" / "Frontmatter fields" sections carry no EAP notice at all | https://junie.jetbrains.com/docs/junie-cli-subagents.html | "This setting is currently in the Early Access Program. To try it, install the Early Access version of Junie CLI." (this sentence sits only under "Configure subagent usage" — the model-selection-policy setting — not under "Creating a custom subagent") | doc snapshot dated 25 September 2026 |
| 2 | Agent file locations (paths grim would target) | `src/install/vendor_junie.rs` (`.junie/agents/*.md` only) | only `.junie/agents/*.md` documented | **expanded** — upstream now also documents `<projectRoot>/.agents/`, `~/.junie/agents/`, and `~/.agents/` (or `%USERPROFILE%` equivalents) as subagent search locations, plus auto-import detection of `.cursor/agents/`, `.claude/agents/`, `.codex/agents/` into `.junie/agents/` | https://junie.jetbrains.com/docs/junie-cli-subagents.html | "Junie CLI looks for custom subagent \*.md files in the following locations: Project scope: `<projectRoot>/.junie/agents/`. Project scope: `<projectRoot>/.agents/`. User scope: `~/.junie/agents/`... User scope: `~/.agents/`..." | doc snapshot dated 25 September 2026 |
| 3 | Rule scoping: `.junie/rules/*.md` concatenated with no per-file activation key | `.claude/rules/vendor-capability-watchlist.md` L130 ("verified 2026-07-27"); `src/install/vendor_junie.rs`; `docs/src/content/docs/vendor-metadata.md` L463; `docs/src/content/docs/clients.md` L216-226 | Degraded — flat concatenation, no `paths`/glob key | unchanged | https://junie.jetbrains.com/docs/environment-variables.html | "`.junie/rules/\*.md`. All Markdown files in the rules directory." (no per-file activation, glob, or paths key documented anywhere on the page) | doc snapshot dated 25 September 2026 |
| 4 | Global rules directory: no `~/.junie/rules/` upstream | `.claude/rules/vendor-capability-watchlist.md` L131; `src/install/vendor_junie.rs` (`kind_surface` false at Global); `docs/src/content/docs/clients.md` L228-230 | "no `~/.junie/rules/` documented; only the workspace `.junie/rules/` exists" | **partially changed — still no `~/.junie/rules/` directory, but upstream now documents a different global mechanism: `~/.junie/AGENTS.md`, merged with project guidelines.** This does not satisfy the row's literal condition (a *rules directory*), since it is a single global file feeding the AGENTS.md/guidelines pipeline, not the `.junie/rules/*.md` per-file surface grim's Rule kind maps to — flag for owner judgment, not an automatic flip | https://junie.jetbrains.com/docs/guidelines-and-memory.html | "Beyond project-level settings, users can establish system-wide rules at `~/.junie/AGENTS.md`... If both global and project guidelines exist, Junie includes both and marks them clearly." | doc snapshot dated 25 September 2026 |
| 5 | MCP env interpolation undocumented (JUNIE-2173) | `.claude/rules/vendor-capability-watchlist.md` L132; `src/install/vendor_junie.rs` (`has_env_refs()` skip) | "env interpolation undocumented (JUNIE-2173)" | unchanged — MCP config docs still show only a literal `"env": {"ENV_VAR": "value"}` example with no `${VAR}`/substitution syntax described | https://junie.jetbrains.com/docs/junie-cli-mcp-configuration.html | `"env": { "ENV_VAR": "value" }` (page gives no explanation of interpolation/substitution behavior) | doc snapshot dated 25 September 2026 |
| 6 | `JUNIE_*_LOCATIONS` per-kind override family | `.claude/rules/vendor-capability-watchlist.md` L133; `src/install/vendor_junie.rs` module doc ("not tested in wave 1"); AGENTS.md domains.md config-root variable | "per-kind override family untested" | **changed — the family is now fully documented** (`JUNIE_MODEL_LOCATIONS`, `JUNIE_MCP_LOCATIONS`, `JUNIE_SKILL_LOCATIONS`, `JUNIE_COMMAND_LOCATIONS`, `JUNIE_AGENT_LOCATIONS`, plus matching `*_DEFAULT_LOCATIONS` toggles). No `JUNIE_RULE_LOCATIONS`/`JUNIE_RULES_LOCATIONS` exists — rules are not part of the family. Grim still does not honor any of them | https://junie.jetbrains.com/docs/environment-variables.html | "`JUNIE_SKILL_LOCATIONS` — Additional paths where Junie should search for agent skills. Can be specified multiple times." / "`JUNIE_AGENT_LOCATIONS` — Additional paths where Junie should search for custom agents. Can be specified multiple times." | doc snapshot dated 25 September 2026 |
| 7 | Legacy `guidelines/` folder — no per-file scoping, do not flip to Native | `.claude/rules/vendor-capability-watchlist.md` L134; `src/install/vendor_junie.rs` | "verified 2026-07-26 — NO per-file scoping" | unchanged | https://junie.jetbrains.com/docs/guidelines-and-memory.html | "`.junie/guidelines.md` file or `.junie/guidelines/` folder (legacy format)" (discovery order lists it below `.junie/rules/*.md`, with no per-file key described) | doc snapshot dated 25 September 2026 |
| 8 | Skill paths: `.junie/skills/<name>/` (project) / `~/.junie/skills/<name>/` (global), project overrides same-name user skill | `src/install/vendor_junie.rs`; `docs/src/content/docs/vendor-metadata.md` L382, L406 | as stated | unchanged | https://junie.jetbrains.com/docs/agent-skills.html | "Project scope: `<projectRoot>/.junie/skills/<skill-name>/`" / "User/global scope: `~/.junie/skills/<skill-name>/`" / "the project-level version takes priority and supersedes the user-level skill" | doc snapshot dated 25 September 2026 |
| 9 | MCP config paths `.junie/mcp/mcp.json` (project) / `~/.junie/mcp/mcp.json` (user), key `mcpServers` | `src/install/vendor_junie.rs` | as stated | unchanged | https://junie.jetbrains.com/docs/junie-cli-mcp-configuration.html | "Project scope: `.junie/mcp/mcp.json`... User scope: `~/.junie/mcp/mcp.json`" | doc snapshot dated 25 September 2026 |
| I1 | Sweep: trust-verification gate added before loading MCP servers/skills/commands/project config | none — new, not covered by any existing ledger row | n/a | new upstream behavior in the release window; no grim rendering/doc statement currently covers this (informational — does not change any file grim writes) | https://github.com/JetBrains/junie/releases/tag/2548.5 | "Added project trust verification before loading MCP servers, skills, commands, and project configuration" | build 2548.5 (2026-08-03) |
| I2 | Sweep: `~/.junie/AGENTS.md` global guidelines mechanism | none — new, adjacent to the "Global rules directory" watchlist row (see claim 4) | n/a | new: a global, user-level guidelines file now exists and is merged with project guidelines — grim documents/renders nothing for junie global rules today | https://junie.jetbrains.com/docs/guidelines-and-memory.html | "users can establish system-wide rules at `~/.junie/AGENTS.md` (or `%USERPROFILE%\.junie\AGENTS.md` on Windows)" | doc snapshot dated 25 September 2026 |

Unsourced:
- Whether the "Configure subagent usage" EAP setting has since reached GA (i.e. whether *any* remaining sliver of the subagent surface is still EAP) — `junie-cli-eap.html` lists no named EAP features at all, so there is no primary text confirming or denying GA for that specific setting beyond the subagents page's own EAP sentence (claim 1 uses that sentence as-is).
- YouTrack issue JUNIE-2173 itself (the watchlist's citation for "MCP env interpolation undocumented") — youtrack.jetbrains.com/issue/JUNIE-2173 returned no fetchable content (JS-rendered, empty via WebFetch), and it is not a GitHub issue (JetBrains/junie has no issue #2173 matching). Verified the underlying claim instead via the current MCP config doc (claim 5).
- GitHub `JetBrains/junie` release changelog for entries between 2026-07-17 and 2026-09-27 that touch skills/rules/agents/MCP/hooks/frontmatter beyond I1 — checked every release body in that window (`gh api --paginate repos/JetBrains/junie/releases`); all other entries are model-support additions or unrelated UI features (`/review`, `/goal`, `/demo`, `/voice`, `/stats`, streaming, `/branch`), none touching grim-relevant surfaces.

Feed notes:
- Newest tag or heading seen: release `3419.7` / "Release 26.9.22 (3419.7)" on the `whats-new` page (Sep 22, 2026); newest GitHub release tag is `3531.1` (2026-09-25T21:09:45Z, nightly build with no changelog body). Cursor found: n/a — cursor was `—`.
- Vendor version current today: build `3531.1` (GitHub releases, nightly, 2026-09-25) / stable-channel `26.9.22 (3419.7)` per https://junie.jetbrains.com/whats-new (Sep 22, 2026).
- Anything grim renders or documents that the feed shows changed but no ledger row covers: the subagent file-location expansion to `.agents/`/`~/.agents/` and cross-vendor auto-import (claim 2), and the new `~/.junie/AGENTS.md` global-guidelines mechanism (I2) — neither has a grim rendering/doc statement today.

## Verifier sign-off

One opus verifier (C-008) re-fetched each claim that drives a docs, comment,
watchlist-wording or issue-draft change. No claim in this pass drives a
renderer, metadata or validation change unless Routing says so.

| Claim | Verdict | Evidence URL | Verbatim quote / note |
|---|---|---|---|
| V7 junie subagents EAP | CONFIRMED | https://junie.jetbrains.com/docs/junie-cli-subagents.html | The only EAP sentence, under "Configure subagent usage" (the Subagents model-selection setting): "This setting is currently in the Early Access Program (EAP). To try it, install the Early Access version of Junie CLI." "Creating a custom subagent", "File location" and "Frontmatter fields" carry no EAP notice. Locations `<projectRoot>/.junie/agents/`, `<projectRoot>/.agents/`, `~/.junie/agents/`, `~/.agents/` match. |
| V8 junie global AGENTS.md | CONFIRMED | https://junie.jetbrains.com/docs/guidelines-and-memory.html | "In addition to project-level guidelines, Junie CLI also supports global guidelines from `~/.junie/AGENTS.md`." "If both global and project guidelines exist, Junie includes both and marks them clearly." Only project `.junie/rules/*.md` is mentioned; there is no `~/.junie/rules/`. |
| V9 junie env vars | CONFIRMED | https://junie.jetbrains.com/docs/environment-variables.html | JUNIE_SKILL_LOCATIONS: "Additional paths where Junie should search for agent skills. Can be specified multiple times." JUNIE_SKILL_DEFAULT_LOCATIONS: "Enable or disable default skill locations (per-user and per-project). Defaults to `true`." The AGENT and MCP pairs use the same wording. There is no rules-location variable (only JUNIE_GUIDELINES_FILENAME). Note: separately, JUNIE_HOME does relocate: "Home directory for Junie CLI. Overrides the default `~/.junie`." |
| V10 junie MCP ${VAR} | CONFIRMED | https://junie.jetbrains.com/docs/junie-cli-mcp-configuration.html | No interpolation syntax is documented. Env vars appear only as literals (`"env": { "ENV_VAR": "value" }`) and in "avoid sharing secrets or sensitive environment variables if the `.junie/mcp/mcp.json` file is committed to version control". |

## Live CLI check

n/a — `sweep` depth on a Tier 2 harness (the live check is `deep` only).

## Routing

| Claim | Class | Landed as |
|---|---|---|
| 1, 2 / V7 subagents out of EAP | (a) + (c) | watchlist row (condition met); `vendor_junie.rs` module doc; issue draft 1 |
| 4, I2 / V8 `~/.junie/AGENTS.md` | (a) | watchlist row: a single global file, not a rules directory — the `kind_surface` override stays |
| 6 / V9 `JUNIE_*_LOCATIONS`, `JUNIE_HOME` | (a) + (c) | watchlist row; `vendor-metadata.md` env cell; module doc; issue draft 2 |
| 5 / V10 MCP env interpolation | (a) | row dated, still undocumented |
| 3, 7, 8, 9 | (a) | confirmed unchanged; stamps dated |
| I1 trust gate (2548.5) | finding | project skills and MCP load only in a trusted project; grim's files are unchanged |

Security: none found.

## Issue drafts

Filed as https://github.com/grimoire-rs/grimoire/issues/148.

### 1. Junie: enable the Agent kind — subagents are no longer EAP

**Labels:** `enhancement`, `vendor:junie`, `needs-design`

The Junie subagents page documents custom subagents without an EAP gate;
the only EAP sentence covers the model-selection setting
(<https://junie.jetbrains.com/docs/junie-cli-subagents.html>, 2026-09-25
snapshot, verified 2026-09-27). Search locations: `<projectRoot>/.junie/agents/`,
`<projectRoot>/.agents/`, `~/.junie/agents/`, `~/.agents/`. The watchlist's
flip condition ("enable Agent at GA") is met. Enabling a kind changes the
`KindSupport` grid and the ADR mapping table that
`kind_support_grid_matches_adr_mapping_table` binds, needs a frontmatter
projection and a `junie.*` agent registry decision, and turns a skipped
install into a written file for existing lockfiles, which needs an upgrade
note. Target `.junie/agents/` (not the shared `.agents/`, which several
vendors read).

### 2. Junie: `JUNIE_HOME` relocates `~/.junie` and grim does not follow it

**Labels:** `vendor:junie`, `layout`

"`JUNIE_HOME` — Home directory for Junie CLI. Overrides the default
`~/.junie`" (<https://junie.jetbrains.com/docs/environment-variables.html>,
verified 2026-09-27). grim writes global skills and MCP under `~/.junie`, so a
user who sets it gets output Junie does not read. Honoring it moves global
output for those users: a layout change needing state migration, a reaper
and an upgrade fixture (Principle 9). The `JUNIE_*_LOCATIONS` family only
adds search paths and needs nothing.

## Friction

- The Junie GH feed's newest tags are nightlies with empty bodies; the `whats-new` page carries the readable headings. Noted in domains.md.
- YouTrack JUNIE-2173 is JS-rendered and returned nothing; the claim was verified against the MCP docs page instead.
- Researcher sandboxes denied network `curl` (and, for some, `gh`); the brief's "`curl -sL <url>` is fine" tip was wrong there. Researchers fell back to WebFetch, whose text is model-extracted, so quotes from vendor sites are summarizer-sourced; GitHub sources were read raw via `gh api` where allowed. Brief fixed.
- Driver cost: the brief told researchers to return only the table, so a 14-target run would have re-typed every table. This run gave each researcher one scratchpad output file and assembled the artifacts from it. Brief now names that file.

## Skill changes

One `chore(skills):` commit for the whole WP-G run, [f9effcc1](https://github.com/grimoire-rs/grimoire/commit/f9effcc1): researcher-brief scratchpad output file, no-curl fallback and re-fetch warning; domains.md docs and feed URLs (kiro, junie, amp, antigravity, goose, openclaw, qoder), `JUNIE_HOME` / `OPENCLAW_HOME` cells, section-default and stamp re-dating note; SKILL.md one artifact per target in a `--domain` run.
