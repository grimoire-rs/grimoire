# Research: upstream refresh — antigravity (sweep)

## Metadata
**Date:** 2026-09-27 · **Domain:** vendors · **Depth:** `antigravity sweep never checked`
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

Vendor version at check: Antigravity **2.17.0** (2026-09-22). Feed: page
<https://antigravity.google/changelog/>; cursor was `—`, **2.17.0**
recorded. Changelog 2.3.0 → 2.17.0 swept. The rules page moved from
`/docs/rules-workflows` to `/docs/rules`.

## Claims

Researcher (sonnet) table, verbatim. Numbered rows re-verify ledger claims;
`I` rows are changelog or surface differences found in the sweep.

| # | Claim | Ledger location | Old value | New value | Source URL | Verbatim quote | Vendor version |
|---|---|---|---|---|---|---|---|
| 1 | Antigravity CLI variant still unserved (separate global skills root) | `.claude/rules/vendor-capability-watchlist.md:135` | "individual-tier Gemini CLI sunset 2026-06-18 → Antigravity CLI... The Antigravity CLI variant is still unserved" | unchanged — CLI still reads its own separate global root, distinct from the 2.0 client grim targets | https://antigravity.google/docs/skills | "~/.gemini/antigravity-cli/skills/<skill-folder>/" | Antigravity CLI (undated on page); Antigravity 2.0 product is at 2.17.0 (2026-09-22) |
| 2 | Rules declined: no global per-file rules dir, no documented frontmatter scoping key | `.claude/rules/vendor-capability-watchlist.md:146` | "declined... (b) no rule-file frontmatter table is published, so `paths` has no on-disk target" | **"Action when shipped" condition MET on both fronts** — a global per-file rules dir now exists AND a documented glob-scoping frontmatter key now exists | https://antigravity.google/docs/rules (redirected from `/docs/rules-workflows`) | "Modular global rules (YAML frontmatter required): `~/.gemini/config/rules/*.md`" / "`trigger` \| `string` \| Yes \| Controls how and when the rule activates: `model_decision`, `always_on`, `glob`, or `manual`." / "`globs` (or `glob`) \| `string` \| Required for `glob` \| Comma-separated file glob patterns" | n/a on page; product current 2.17.0 (2026-09-22) |
| 3 | Project-scope detection: no product-specific project marker documented | `.claude/rules/vendor-capability-watchlist.md:147` | "never detected at project scope... Upstream documents no product-specific project marker" | still not met — a new project config file appeared, but it is **not** product-specific: it nests inside the shared `.gemini/` folder that [Gemini]'s own project detection already keys on, so it cannot be adopted as an Antigravity-exclusive marker | https://antigravity.google/changelog/ | "Repository customization settings now load from `/.gemini/config.json`. The older `.agents/settings.json` is no longer read — move any `personal_customization_dir` entry into the new file." | 2.17.0 (2026-09-22) |
| 4 | Global root sharing: unresolved whether the IDE variant *or* plain Gemini CLI also creates `~/.gemini/config` | `.claude/rules/vendor-capability-watchlist.md:148` | "Unresolved on two fronts: whether the v2.1.x IDE also creates it..., and whether plain Gemini CLI ever creates a `config` subdir" | **IDE front now confirmed** — the Antigravity IDE documents the same `~/.gemini/config/skills` root (with legacy fallback). Gemini CLI front still unconfirmed | https://antigravity.google/docs/skills | "Antigravity IDE - Global: `~/.gemini/config/skills/<skill-folder>/` (with legacy support for `~/.gemini/antigravity/skills/`)" | Antigravity IDE (v2.1.x per module doc; undated on page) |
| 5 | Reverse detection leak into Gemini via `~/.gemini/config` nesting | `.claude/rules/vendor-capability-watchlist.md:149` | "none — disclosed... revisit with `vendor_gemini`'s owner" | unchanged — mechanism reconfirmed (see claim 4), not resolved; still an owner call | https://antigravity.google/docs/skills | "Antigravity IDE - Global: `~/.gemini/config/skills/<skill-folder>/` (with legacy support for `~/.gemini/antigravity/skills/`)" | Antigravity IDE (undated on page) |
| 6 | `ws` MCP transport: ambiguous, raw page unconfirmed | `.claude/rules/vendor-capability-watchlist.md:150` | "`/docs/mcp` names websocket alongside sse/http under one `serverUrl` field, but only via a summarizing fetch — the raw page body could not be retrieved" | unchanged — same sentence reconfirmed via a second independent fetch pass, but curl/raw-page access is blocked in this sandbox (network `Bash` calls denied), so it is still not a raw-text confirmation | https://antigravity.google/docs/mcp | "When declaring remote SSE, Streamable HTTP, or websocket-based MCP connections, you must define the `serverUrl` field." | n/a on page |
| 7 | `antigravity.*` agent registry is empty; upstream documents `mainAgent`/`subagent`/`commandExecutionPolicy`/`mcpServers`/`skills`/`plugins` | `.claude/rules/vendor-capability-watchlist.md:152` | "Upstream `/docs/subagents` also documents `mainAgent`, `subagent`, `commandExecutionPolicy`, `mcpServers`, `skills`/`plugins`" | **list grows** — three more agent-frontmatter keys shipped since verification: `hooks`, `rules`, `inheritCustomizations` (see sweep rows I1–I3 below for each) | https://antigravity.google/changelog/ | "Custom agents can declare their own hooks. A Markdown agent can list hook files in its front matter with a `hooks:` entry." | 2.17.0 (2026-09-22) |
| 8 | MCP `oauth` block shape `{clientId, clientSecret}` + `authProviderType` | `.claude/rules/vendor-capability-watchlist.md:153` | "Upstream shape is `{clientId, clientSecret}` + `authProviderType`" | unchanged — confirmed, with added detail: DCR needs no oauth block at all, manual mode needs exactly `clientId`/`clientSecret` | https://antigravity.google/docs/mcp | "Dynamic Client Registration (DCR) - no additional config needed beyond `serverUrl`" / "Manual credentials - provide: `clientId`, `clientSecret`" | n/a on page |

## Sweep additions (changelog 2026-07-17 → 2026-09-27)

| # | Claim | Ledger location | Old value | New value | Source URL | Verbatim quote | Vendor version |
|---|---|---|---|---|---|---|---|
| I1 | Custom agents can declare hooks via a new `hooks:` frontmatter entry | none — extends row 152's registry candidate list | n/a | new key, not in grim's `antigravity.*` registry or projected fields | https://antigravity.google/changelog/ | "Custom agents can declare their own hooks. A Markdown agent can list hook files in its front matter with a `hooks:` entry." | 2.17.0 (2026-09-22) |
| I2 | `rules:` key in custom markdown agent frontmatter, binding specific rule files to an agent | none — extends row 152's registry candidate list | n/a | new key, not in grim's `antigravity.*` registry or projected fields | https://antigravity.google/changelog/ | "Added support for defining a `rules:` key in custom markdown agent frontmatter to bind specific rule files directly to an agent." | 2.11.0 (2026-08-26) |
| I3 | `inheritCustomizations` agent frontmatter setting to reuse existing skills/rules/subagents | none — extends row 152's registry candidate list | n/a | new key, not in grim's `antigravity.*` registry or projected fields | https://antigravity.google/changelog/ | "Custom agents can now reuse the skills, rules, and subagents you already have, using a single `inheritCustomizations` setting in the agent's frontmatter." | 2.9.1 (2026-08-20) |
| I4 | `.agents/rules.json` (and `skills.json`/`agents.json`) manifests for cross-directory/nested discovery | none — new mechanism, not covered by any ledger row | n/a | new discovery mechanism grim's flat-directory install does not use or account for | https://antigravity.google/docs/rules | "To share rules stored outside `.agents/rules/` or to include nested subdirectories of rules while preserving their YAML frontmatter and triggers, create `.agents/rules.json`" | n/a on page |
| I5 | `@path/to/file` syntax support added inside `AGENTS.md` and custom rule files | none — new syntax feature, not covered by any ledger row | n/a | new syntax grim's plain-copy render does not need to emit, but worth knowing rule bodies can now reference other files | https://antigravity.google/changelog/ | "`@path/to/file` syntax support in `AGENTS.md` and custom rule files" | 2.11.0 (2026-08-26) |

Unsourced:
- Row 151 (path-relocating env override) — re-checked `/docs/subagents`, `/docs/skills`, `/docs/mcp`, `/docs/projects`, `/docs/rules` via WebFetch; no `ANTIGRAVITY_*`-style relocation variable is mentioned on any of them, consistent with the existing "not found" verdict. A negative has no quotable primary sentence, so not placed in the table as a changed row.
- Row 154 (MCP env-var `${VAR}` substitution) — re-checked `/docs/mcp` via WebFetch; still no `${VAR}` substitution documented (consistent with the existing "silence, not a documented negative" verdict). Same reasoning: a negative has no quotable primary text.
- Row 155 (CLI/IDE skills convergence) — tried to find a CLI-specific changelog naming skills moving to the shared `~/.gemini/config/` root (the way `/hooks` and `/agents` did in CLI v1.0.8/v1.1.0); the only changelog fetched (`https://antigravity.google/changelog/`) covers the Antigravity 2.0 desktop product's own versioning (2.3.0–2.17.0), not the CLI's separate version line, so no primary source was found either way.
- `/docs/mcp` raw page body — the brief's fetch tip (raw `curl`, `<page>.md`) could not be executed: this session's `Bash` tool denies all `curl`/network commands (including with `dangerouslyDisableSandbox`), so every quote above came through `WebFetch`'s summarizing fetch, not a verified raw body. This is the same limitation the existing `ws`-transport watchlist row already flags.

Feed notes:
- Newest tag or heading seen: **Version 2.17.0 — September 22, 2026** ("Plan before you build"). Cursor found: n/a — cursor was `—`.
- Vendor version current today: **2.17.0** (2026-09-22), source https://antigravity.google/changelog/.
- Anything grim renders or documents that the feed shows changed but no ledger row covers: (1) the workspace project-config file moved from `.agents/settings.json` to `/.gemini/config.json` (2.17.0) — collides with Gemini's existing `.gemini/` project-detection marker, see claim 3; (2) three new agent-frontmatter keys (`hooks`, `rules`, `inheritCustomizations` — sweep I1–I3); (3) `.agents/rules.json`/`skills.json`/`agents.json` nested-discovery manifests (sweep I4); (4) `@path/to/file` reference syntax inside rule/AGENTS.md bodies (sweep I5). The single highest-priority item for a maintainer to act on is **claim 2**: the Rules-declined watchlist row's stated flip condition is now met on both fronts (global per-file dir + documented `trigger`/`globs` scoping key).

## Verifier sign-off

One opus verifier (C-008) re-fetched each claim that drives a docs, comment,
watchlist-wording or issue-draft change. No claim in this pass drives a
renderer, metadata or validation change unless Routing says so.

| Claim | Verdict | Evidence URL | Verbatim quote / note |
|---|---|---|---|
| V12 | CONFIRMED | https://antigravity.google/docs/rules | Files in `~/.gemini/config/rules/*.md` require "YAML frontmatter"; `globs` takes "comma-separated file glob patterns (for example, `"*.py, *_test.py"`)" and is "required for glob". Triggers `model_decision`/`always_on`/`glob`/`manual`; workspace "`.agents/rules/*.md` in your workspace root or any project subdirectory". Also: singular `glob:` accepted; `~/.gemini/{,config/}{AGENTS,GEMINI}.md` "do not use frontmatter". |
| V13 | CONFIRMED | https://antigravity.google/docs/skills | IDE global `~/.gemini/config/skills/<skill-folder>/`, "Legacy global: `~/.gemini/antigravity/skills/` (still supported)"; CLI global `~/.gemini/antigravity-cli/skills/<skill-folder>/`. Also CLI plugin skills `~/.gemini/antigravity-cli/plugins/<name>/skills/`; workspace `.agents/skills` (`.agent/skills` back-compat). |
| V14 | CONFIRMED | https://antigravity.google/changelog | 2.17.0 (2026-09-22): "Repository customization settings now load from `/.gemini/config.json`. The older `.agents/settings.json` is no longer read"; "A Markdown agent can list hook files in its front matter with a `hooks:` entry." `rules:` under 2.11.0 (2026-08-26); `inheritCustomizations` under 2.9.1 (2026-08-20). Note: changelog writes the path with a leading slash (`/.gemini/config.json`, i.e. repo-root). |

## Live CLI check

n/a — `sweep` depth on a Tier 2 harness (the live check is `deep` only).

## Routing

| Claim | Class | Landed as |
|---|---|---|
| 2 / V12 global per-file rules + `trigger`/`globs` | (a) + (c) | watchlist row (both flip conditions met); issue draft 1 |
| 4, 5 / V13 IDE shares `~/.gemini/config` | (a) | rows dated; the Gemini CLI half stays unconfirmed |
| 3 / V14 `.gemini/config.json` | (a) | project-detection row: the new file sits in Gemini's own `.gemini/`, so it is still not a product marker |
| 7, I1–I3 / V14 agent keys | (a) | registry-candidate row lists `hooks`, `rules`, `inheritCustomizations` |
| I4 `.agents/rules.json` manifests, I5 `@path` | finding | grim writes flat files and plain bodies; nothing moves |
| 1, 6, 8 and rows 151, 154, 155 | (a) | dated, unchanged |

Security: none found.

## Issue drafts

Filed as https://github.com/grimoire-rs/grimoire/issues/139.

### 1. Antigravity: enable the Rule kind — per-file scoping shipped

**Labels:** `enhancement`, `vendor:antigravity`, `needs-design`

Antigravity 2.x documents global per-file rules at `~/.gemini/config/rules/*.md`
(frontmatter required) and workspace `.agents/rules/*.md`, with frontmatter
`trigger` = `model_decision` | `always_on` | `glob` | `manual` and `globs`
("comma-separated file glob patterns") required for `glob`
(<https://antigravity.google/docs/rules>, verified 2026-09-27, 2.17.0). Both
watchlist flip conditions are met. Design points:

- `paths` → `trigger: glob` + comma-joined `globs`, unscoped →
  `trigger: always_on`. A comma inside a glob splits it, as for Cursor
  (`clients.md#gap-cursor-globs`).
- Workspace `.agents/rules/` is under the five-client `.agents/` tree; check
  that no other client reads `.agents/rules/`.
- Global `~/.gemini/config/rules/` nests in Gemini CLI's root (the reverse
  detection leak row).
- Enabling a kind changes the `KindSupport` grid, the ADR mapping table and
  the docs matrix, and turns skips into installs (upgrade note).

## Friction

- Raw fetches of antigravity.google were denied in the research sandbox; `/docs/mcp` is still confirmed only through a summarizing fetch.
- domains.md named `/docs/rules-workflows`; it redirects to `/docs/rules`. Fixed in domains.md.
- Researcher sandboxes denied network `curl` (and, for some, `gh`); the brief's "`curl -sL <url>` is fine" tip was wrong there. Researchers fell back to WebFetch, whose text is model-extracted, so quotes from vendor sites are summarizer-sourced; GitHub sources were read raw via `gh api` where allowed. Brief fixed.
- Driver cost: the brief told researchers to return only the table, so a 14-target run would have re-typed every table. This run gave each researcher one scratchpad output file and assembled the artifacts from it. Brief now names that file.

## Skill changes

One `chore(skills):` commit for the whole WP-G run, [f9effcc1](https://github.com/grimoire-rs/grimoire/commit/f9effcc1): researcher-brief scratchpad output file, no-curl fallback and re-fetch warning; domains.md docs and feed URLs (kiro, junie, amp, antigravity, goose, openclaw, qoder), `JUNIE_HOME` / `OPENCLAW_HOME` cells, section-default and stamp re-dating note; SKILL.md one artifact per target in a `--domain` run.
