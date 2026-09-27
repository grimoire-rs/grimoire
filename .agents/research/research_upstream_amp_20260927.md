# Research: upstream refresh — amp (sweep)

## Metadata
**Date:** 2026-09-27 · **Domain:** vendors · **Depth:** `amp sweep never checked`
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

Vendor version at check: none published (compiled binary, no public repo,
no version on any docs page). Feed: page <https://ampcode.com/chronicle>;
cursor was `—`, newest heading **Less Noise** (2026-09-25) recorded.
Relevant entries 2026-07-17 → 2026-09-27: "Global Plugins and Skills"
(2026-08-11) and "MCP in Orbs" (2026-08-19); their article pages failed to
fetch, so the claims rest on the docs pages. The manual moved from
`ampcode.com/manual` to `ampcode.com/docs/markdown/…`.

## Claims

Researcher (sonnet) table, verbatim. Numbered rows re-verify ledger claims;
`I` rows are changelog or surface differences found in the sweep.

| # | Claim | Ledger location | Old value | New value | Source URL | Verbatim quote | Vendor version |
|---|---|---|---|---|---|---|---|
| 1 | `$AMP_SETTINGS_FILE` honored / documented | `.claude/rules/vendor-capability-watchlist.md:141` | not honored — contested, NOT confirmed nonexistent | unchanged — still absent from the docs, `--settings-file` still the only documented mechanism | https://ampcode.com/docs/markdown/cli/settings | "Custom user settings: pass `--settings-file <path>` to point Amp at a different user settings file" (page confirmed to contain no occurrence of "AMP_SETTINGS_FILE" or "XDG_CONFIG_HOME") | not stated |
| 2 | `$XDG_CONFIG_HOME` per platform (Amp) | `.claude/rules/vendor-capability-watchlist.md:142` | UNRESOLVED at every tier — manual greps to zero "XDG" hits | unchanged — the successor settings page also has zero "XDG_CONFIG_HOME" hits; still undocumented | https://ampcode.com/docs/markdown/cli/settings | "When the same setting appears in multiple places, workspace settings override user settings." (no XDG mention anywhere on the page) | not stated |
| 3 | Skills-scan precedence (Amp) | `.claude/rules/vendor-capability-watchlist.md:144` | grim writes the shared pool's `~/.agents/skills/` — middle of 3 global dirs (`~/.config/agents/skills` highest, `~/.config/amp/skills` lowest); "not a visibility bug" | New value: the manual now documents 11 precedence tiers, not 3 — after the 3 global dirs come project `.agents/skills/`, `.claude/skills/`, `~/.claude/skills/`, `~/.claude/plugins/cache/`, `amp.skills.path` dirs, built-in, personal repo, workspace repo. It also documents a config key `amp.skills.disableGlobalAgentsSkills` that **fully skips** `~/.config/agents/skills/` *and* `~/.agents/skills/` (grim's write target) when set — the "only a tie can be lost" claim no longer covers this opt-out | https://ampcode.com/docs/markdown/customize/skills | "Amp uses the first skill with a given frontmatter `name`. The order is: 1. `~/.config/agents/skills/` 2. `~/.agents/skills/` 3. `~/.config/amp/skills/` 4. `.agents/skills/` in the project and searched parent directories ... Set `amp.skills.disableClaudeCodeSkills` ... or `amp.skills.disableGlobalAgentsSkills` to skip `~/.config/agents/skills/` and `~/.agents/skills/`." | not stated |
| 4 | Skills shared-pool 3-dir global order | `src/install/vendor_amp.rs` module doc | `~/.config/agents/skills` (highest) → `~/.agents/skills` (grim's) → `~/.config/amp/skills` | unchanged — same 3-dir order confirmed as items 1–3 of the now-longer list (see claim 3) | https://ampcode.com/docs/markdown/customize/skills | "1. `~/.config/agents/skills/` 2. `~/.agents/skills/` 3. `~/.config/amp/skills/`" | not stated |
| 5 | MCP config shape: `amp.mcpServers` key, `${VAR}` env refs, workspace-over-global precedence | `src/install/vendor_amp.rs` module doc | project `.amp/settings.json` (workspace tier, merged over global) / global `~/.config/amp/settings.json`, key `"amp.mcpServers"`, env refs `${VAR_NAME}` | unchanged — key, env-ref syntax and precedence direction confirmed as-is | https://ampcode.com/docs/markdown/customize/mcp | "project servers override workspace servers with the same ID, and workspace servers override personal servers with the same ID" ("url": "${SRC_ENDPOINT}/.api/mcp/v1" given as the env-ref example) | not stated |
| 6 | Rules declined: only instruction surface is `AGENTS.md`→`AGENT.md`→`CLAUDE.md` | `src/install/vendor_amp.rs` module doc | AGENTS.md (→AGENT.md→CLAUDE.md) only; wave-2 adds `@`-mention + `globs:` frontmatter scoped injection | unchanged — fallback order and `@`-mention/`globs` scoping both confirmed live in current docs | https://ampcode.com/docs/markdown/customize/agents-md | "If no `AGENTS.md` exists in a directory, but a file named `AGENT.md` (without an `S`) or `CLAUDE.md` does exist, that file will be included." | not stated |
| 7 | `docs/src/content/docs/vendor-metadata.md:410` — Amp skills pool path + `$AMP_SETTINGS_FILE` contested note | `docs/src/content/docs/vendor-metadata.md:410` | `$HOME/.agents/skills/<name>/` (shared pool); "`$AMP_SETTINGS_FILE` is not honored either; its existence is contested rather than disproven" | unchanged | https://ampcode.com/docs/markdown/cli/settings | "Custom user settings: pass `--settings-file <path>` to point Amp at a different user settings file" | not stated |
| 8 | `docs/src/content/docs/clients.md:302-306` — "Amp's only instruction surface is `AGENTS.md` (falling back to `AGENT.md`, then `CLAUDE.md`)... subagents spawned at runtime with no installable file format" | `docs/src/content/docs/clients.md:302-306` | as quoted above | unchanged — fallback order confirmed; no dedicated subagents/file-format doc page found (404 on `customize/subagents`), consistent with "no installable file format" | https://ampcode.com/docs/markdown/customize/agents-md | "If no `AGENTS.md` exists in a directory, but a file named `AGENT.md` (without an `S`) or `CLAUDE.md` does exist, that file will be included." | not stated |
| I1 | New skills config keys `amp.skills.disableClaudeCodeSkills` / `amp.skills.disableGlobalAgentsSkills` (from the "Global Plugins and Skills" chronicle entry, 2026-08-11) | none — not covered by any current ledger row | n/a (undocumented in grim today) | Two settings now exist that change which skill dirs Amp scans; `disableGlobalAgentsSkills` fully removes grim's global write target (`~/.agents/skills/`) from the scan when a user sets it | https://ampcode.com/docs/markdown/customize/skills | "Set `amp.skills.disableClaudeCodeSkills` in Configuration to skip the Claude-compatible locations, or `amp.skills.disableGlobalAgentsSkills` to skip `~/.config/agents/skills/` and `~/.agents/skills/`." | not stated |

Unsourced:
- Chronicle "Global Plugins and Skills" article text (https://ampcode.com/news/global-plugins-and-skills) — WebFetch returned "Parse Error: Header overflow" on every attempt (also on the `.md` variant, which 404'd); could not get its own verbatim primary text, so claim I1 is sourced from the docs/markdown/customize/skills page instead, not this article.
- Whether Amp's MCP "personal/workspace/project" remote-definition scopes (ampcode.com/docs/markdown/customize/mcp, "MCP in Orbs" chronicle entry, 2026-08-19) apply to the local `.amp/settings.json` grim writes, or are a separate cloud-only (Orbs) feature — page text suggests the latter ("Remote MCP definitions are available across Amp clients without a local settings file") but no source states this explicitly enough to quote as confirmed; not put in the table since it doesn't change what grim renders/documents.
- Current Amp CLI/binary version number — no public repo, no releases API (`ai-amp-cli` is third-party), and no version string found on any fetched docs/chronicle page; never guessed.

Feed notes:
- Newest tag or heading seen: "The Mac App Is Your Runner" (2026-09-24) is the newest dated Chronicle entry before "Less Noise" (2026-09-25, undated body beyond the heading date). Cursor found: n/a — cursor was `—`.
- Vendor version current today: unknown — no version string is published anywhere in Amp's docs or chronicle; Amp ships as a compiled binary with no public repo or releases feed (confirmed by `src/install/vendor_amp.rs`'s own module doc, unchanged).
- Anything grim renders or documents that the feed shows changed but no ledger row covers: the skills precedence list is now far longer than the 3-dir order grim's docs describe, and `amp.skills.disableGlobalAgentsSkills` can zero out grim's global skill output entirely — see row 3 / I1 above (https://ampcode.com/docs/markdown/customize/skills). The chronicle's "MCP in Orbs" (2026-08-19) and "Global Plugins and Skills" (2026-08-11) entries are the likely triggers but their own article pages could not be fetched (see Unsourced).

## Verifier sign-off

One opus verifier (C-008) re-fetched each claim that drives a docs, comment,
watchlist-wording or issue-draft change. No claim in this pass drives a
renderer, metadata or validation change unless Routing says so.

| Claim | Verdict | Evidence URL | Verbatim quote / note |
|---|---|---|---|
| V23 | CONFIRMED | https://ampcode.com/docs/markdown/customize/skills | `amp.skills.disableGlobalAgentsSkills` skips "`~/.config/agents/skills/` and `~/.agents/skills/`"; precedence 1. `~/.config/agents/skills/`, 2. `~/.agents/skills/`, 3. `~/.config/amp/skills/`, then project `.agents/skills/`, `.claude/skills/`… |

## Live CLI check

n/a — `sweep` depth on a Tier 2 harness (the live check is `deep` only).

## Routing

| Claim | Class | Landed as |
|---|---|---|
| 3, I1 / V23 skills precedence, `disableGlobalAgentsSkills` | (a) | watchlist row: an opt-out removes grim's pool target from Amp's scan; disclosed there |
| 1, 2, 7 `AMP_SETTINGS_FILE`, XDG | (a) | rows dated, still undocumented |
| 4, 5, 6, 8 | (a) | confirmed unchanged; stamp dated |

Security: none found.

## Issue drafts

none

## Friction

- Amp chronicle article pages returned "Header overflow" to WebFetch on every attempt; claims were sourced from the docs pages instead.
- domains.md named `ampcode.com/manual` as the docs; it moved to `ampcode.com/docs/markdown/`. Fixed in domains.md.
- Researcher sandboxes denied network `curl` (and, for some, `gh`); the brief's "`curl -sL <url>` is fine" tip was wrong there. Researchers fell back to WebFetch, whose text is model-extracted, so quotes from vendor sites are summarizer-sourced; GitHub sources were read raw via `gh api` where allowed. Brief fixed.
- Driver cost: the brief told researchers to return only the table, so a 14-target run would have re-typed every table. This run gave each researcher one scratchpad output file and assembled the artifacts from it. Brief now names that file.

## Skill changes

One `chore(skills):` commit for the whole WP-G run, [f9effcc1](https://github.com/grimoire-rs/grimoire/commit/f9effcc1): researcher-brief scratchpad output file, no-curl fallback and re-fetch warning; domains.md docs and feed URLs (kiro, junie, amp, antigravity, goose, openclaw, qoder), `JUNIE_HOME` / `OPENCLAW_HOME` cells, section-default and stamp re-dating note; SKILL.md one artifact per target in a `--domain` run.
