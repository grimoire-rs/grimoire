# Research: upstream refresh — cline (sweep)

## Metadata
**Date:** 2026-09-27 · **Domain:** vendors · **Depth:** `cline sweep never checked`
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

Vendor version at check: extension **v4.1.21**, CLI **v3.0.65** (both
2026-09-24). Feed: GH `cline/cline` releases, extension `v*` and `cli-v*`
streams; cursor was `—`, **v4.1.21, cli-v3.0.65** recorded. CLI release notes
3.0.54 → 3.0.65 swept: global rules also read from `~/Cline/Rules` and
`~/.cline/rules` (3.0.56), and an Agent Plugins Hub under `~/.agents/plugins/*`
(3.0.62).

## Claims

Researcher (sonnet) table, verbatim. Numbered rows re-verify ledger claims;
`I` rows are changelog or surface differences found in the sweep.

| # | Claim | Ledger location | Old value | New value | Source URL | Verbatim quote | Vendor version |
|---|---|---|---|---|---|---|---|
| 1 | Rule kind: `.clinerules/` documents genuine per-file `paths:` frontmatter scoping | `.claude/rules/vendor-capability-watchlist.md:179` | Capability EXISTS — per-file `paths:` frontmatter scoping documented | unchanged | https://docs.cline.bot/features/cline-rules.md | "Workspace rules go in `.clinerules/` or `.cline/rules/` at your project root." / "Currently, `paths` is the supported conditional. It takes an array of glob patterns." | docs (current, undated) |
| 2 | `CLINE_DATA_DIR` override: not honored; evidenced only for Cline's MCP data directory, never for skill discovery | `.claude/rules/vendor-capability-watchlist.md:180` | not honored — no first-party source ties it to skill paths | **CHANGED** — a first-party source now ties it to skill (and rule) paths. Action-when-shipped condition ("honor for skills once a first-party source ties it to skill paths") is now met. Caveat: this `~/.cline/data/` tree is a *separate* doc page from `customization/skills.md`'s `~/.cline/skills/` global path — the two are not reconciled upstream | https://docs.cline.bot/cli/cli-reference.md | "CLINE_DATA_DIR — Custom configuration directory (replaces `~/.cline/data/`)" / "`~/.cline/data/settings/skills/` — # Global skills" / "`~/.cline/data/settings/rules/` — # Global rules" | CLI v3.0.65 (current) |
| 3 | Skill paths: `.cline/skills/<name>/` (project), `~/.cline/skills/<name>/` (global; `%USERPROFILE%\.cline\skills\` on Windows); precedence `.cline/skills/` → `.clinerules/skills/` → `.claude/skills/` | `src/install/vendor_cline.rs` module doc | as stated | unchanged | https://docs.cline.bot/customization/skills.md | "`.cline/skills/` (recommended)" / "`.clinerules/skills/`" / "`.claude/skills/`" (project, listed in that order); "`~/.cline/skills/` (macOS/Linux)" / "`C:\Users\USERNAME\.cline\skills\` (Windows)" (global) | docs (current, undated) |
| 4 | Not a shared-pool client — `.agents/skills` appears in neither the project nor the global skill-path list | `src/install/vendor_cline.rs` module doc | confirmed absence | unchanged for `.agents/skills` itself (still absent from the documented list) — see sweep row I1 for a related but distinct `.agents/`-prefixed surface | https://docs.cline.bot/customization/skills.md | Full documented skill-path list is exactly the five paths quoted in claim 3; none is `.agents/skills` | docs (current, undated) |
| 5 | Agents: declined — no installable subagent file format | `src/install/vendor_cline.rs` module doc | as stated | unchanged | https://docs.cline.bot/cli/agent-teams.md | "Team state is stored at `~/.cline/data/teams/[team-name]/` and includes: Task board with current tasks and status, Inter-agent mailbox, Mission log with activity history" (state only; teams are started with `cline --team-name auth-sprint "..."` or `/team ...`, not an installable file) | CLI v3.0.65 (current) |
| 6 | MCP: declined — no grim-writable config file surface | `src/install/vendor_cline.rs` module doc | no grim-writable config file surface | **CHANGED for the CLI**: the Cline CLI documents a directly-editable global config file at `~/.cline/mcp.json` — the same root grim already owns for Cline's global scope. Still holds for the IDE extension, which documents no on-disk path (UI-managed settings JSON only) | https://docs.cline.bot/mcp/mcp-overview.md | "Edit your MCP config file and add either: **CLI:** `~/.cline/mcp.json` **IDE extensions:** ... This opens the MCP settings JSON used by the extension; add/update entries under `mcpServers`." | docs (current, undated) |
| 7 | `CLINE_DATA_DIR`: "None — evidenced only for the MCP data directory, never for skill discovery, so grim does not honor it" | `docs/src/content/docs/vendor-metadata.md:412` | as stated | **CHANGED** — same finding as claim 2 | https://docs.cline.bot/cli/cli-reference.md | same quotes as claim 2 | CLI v3.0.65 (current) |
| 8 | Cline's rules decline is not about a missing capability; documented non-adopter of the shared `.agents/skills` pool | `docs/src/content/docs/clients.md:361-367` | as stated | unchanged (rules capability per claim 1; `.agents/skills` non-adoption per claim 4) | https://docs.cline.bot/features/cline-rules.md, https://docs.cline.bot/customization/skills.md | same quotes as claims 1 and 4 | docs (current, undated) |
| I1 | Sweep: Agent Plugins Hub adds a global `~/.agents/plugins/*` surface (skills exposed as `plugin-name:skill-name`; plugin MCP servers auto-start); workspace `.agents/plugins` deliberately not scanned; names `cline_mcp_settings.json` as a real config artifact | none — not covered by any existing watchlist row or module-doc claim | n/a | New: a second `.agents/`-prefixed surface exists, distinct from the pool path `.agents/skills`; also confirms `cline_mcp_settings.json` as a named, real MCP config file (bypassed by plugin-sourced servers, still the target for user-added ones) | https://github.com/cline/cline/releases/tag/cli-v3.0.62 | "Agent Plugins are now managed through the Hub. Packages under `~/.agents/plugins/*` are discovered and validated, their skills are exposed through the skills tool as `plugin-name:skill-name`, and their MCP servers start without touching `cline_mcp_settings.json`. ... Workspace `.agents/plugins` directories are deliberately not scanned, so opening a repo cannot implicitly start repo-controlled MCP servers." | CLI v3.0.62 (2026-09-15) |

Unsourced:
- None. Every claim checked was backed by a primary URL and verbatim quote.

Feed notes:
- Newest tag or heading seen: `v4.1.21` (extension, 2026-09-24T16:16:08Z) and `cli-v3.0.65` (CLI, 2026-09-24T05:54:42Z). Cursor found: n/a — cursor was `—`.
- Vendor version current today: extension v4.1.21, CLI v3.0.65 (source: `gh api repos/cline/cline/releases`, accessed 2026-09-27).
- Anything grim renders or documents that the feed shows changed but no ledger row covers: (1) global rules are also read from `~/Cline/Rules` and `~/.cline/rules` in addition to the OS-specific `Documents\Cline\Rules` path (cli-v3.0.54/56 fix notes, e.g. https://github.com/cline/cline/releases/tag/cli-v3.0.56: "Global rules are now also read from `~/Cline/Rules`, which is where the Rules tab writes them on WSL and headless installs whose Documents folder resolves to the home directory."); grim's module doc and docs pages don't enumerate these fallback global-rule paths (moot today since Rule kind is declined for Cline, but relevant the moment claim 1's "strongest decline→support candidate" is acted on). (2) See I1 above (Agent Plugins Hub / `.agents/plugins`).

## Verifier sign-off

One opus verifier (C-008) re-fetched each claim that drives a docs, comment,
watchlist-wording or issue-draft change. No claim in this pass drives a
renderer, metadata or validation change unless Routing says so.

| Claim | Verdict | Evidence URL | Verbatim quote / note |
|---|---|---|---|
| V15 | CONFIRMED | https://docs.cline.bot/cli/cli-reference.md ; https://docs.cline.bot/customization/skills.md | CLI ref: `CLINE_DATA_DIR` = "Custom configuration directory (replaces `~/.cline/data/`)", global skills `~/.cline/data/settings/skills/`. Skills page: global "macOS/Linux: `~/.cline/skills/`" — the two pages genuinely disagree. |
| V16 | CONFIRMED | https://docs.cline.bot/mcp/mcp-overview.md ; https://docs.cline.bot/features/cline-rules.md | "CLI: `~/.cline/mcp.json`"; IDE: "This opens the MCP settings JSON used by the extension; add/update entries under `mcpServers`." Rules: "Currently, `paths` is the supported conditional. It takes an array of glob patterns". |

## Live CLI check

n/a — `sweep` depth on a Tier 2 harness (the live check is `deep` only).

## Routing

| Claim | Class | Landed as |
|---|---|---|
| 2, 7 / V15 `CLINE_DATA_DIR` | (a) + (c) | watchlist row; `vendor-metadata.md` env cell (two upstream pages disagree on the global skills dir); issue draft 2 |
| 6 / V16 CLI `~/.cline/mcp.json` | (a) + (c) | module doc; new `MCP kind` row (Cline, Droid, Warp); issue draft 1 |
| 1 / V16 `paths` conditional | (a) | Rule-kind row dated, still the strongest decline→support candidate |
| 3, 4, 5, 8 | (a) | confirmed unchanged; stamp dated |
| I1 Agent Plugins Hub `~/.agents/plugins` | finding | not the skills pool; workspace plugins are deliberately not scanned |

Security: none found.

## Issue drafts

Filed as https://github.com/grimoire-rs/grimoire/issues/142.

### 1. Cline: enable MCP for the CLI's `~/.cline/mcp.json`?

**Labels:** `enhancement`, `vendor:cline`, `needs-design`

The Cline CLI documents a directly editable MCP file, "CLI: `~/.cline/mcp.json`",
with `mcpServers` (<https://docs.cline.bot/mcp/mcp-overview.md>, verified
2026-09-27, CLI v3.0.65). The IDE extension keeps its MCP settings UI-managed.
grim declines MCP for Cline. A global-only JSON splice reaches CLI users only;
decide whether a CLI-only surface is worth a kind enablement (grid, ADR
mapping table, docs matrix, upgrade note), and check the env-ref form first.

### 2. Cline: `CLINE_DATA_DIR` and the global skills dir disagree upstream

**Labels:** `vendor:cline`, `layout`

The CLI reference says `CLINE_DATA_DIR` "replaces `~/.cline/data/`" and lists
global skills at `~/.cline/data/settings/skills/`
(<https://docs.cline.bot/cli/cli-reference.md>). The skills page lists
`~/.cline/skills/` (<https://docs.cline.bot/customization/skills.md>), which is
where grim writes. The watchlist condition ("honor for skills once a
first-party source ties it to skill paths") is met on paper, but for a
different directory. Settle which dir the CLI and the extension read (live
check), then decide. Any move is a layout change: state migration, reaper,
upgrade fixture.

## Friction

- Researcher sandboxes denied network `curl` (and, for some, `gh`); the brief's "`curl -sL <url>` is fine" tip was wrong there. Researchers fell back to WebFetch, whose text is model-extracted, so quotes from vendor sites are summarizer-sourced; GitHub sources were read raw via `gh api` where allowed. Brief fixed.
- Driver cost: the brief told researchers to return only the table, so a 14-target run would have re-typed every table. This run gave each researcher one scratchpad output file and assembled the artifacts from it. Brief now names that file.

## Skill changes

One `chore(skills):` commit for the whole WP-G run, [f9effcc1](https://github.com/grimoire-rs/grimoire/commit/f9effcc1): researcher-brief scratchpad output file, no-curl fallback and re-fetch warning; domains.md docs and feed URLs (kiro, junie, amp, antigravity, goose, openclaw, qoder), `JUNIE_HOME` / `OPENCLAW_HOME` cells, section-default and stamp re-dating note; SKILL.md one artifact per target in a `--domain` run.
