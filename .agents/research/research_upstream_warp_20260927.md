# Research: upstream refresh — warp (sweep)

## Metadata
**Date:** 2026-09-27 · **Domain:** vendors · **Depth:** `warp sweep never checked`
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

Vendor version at check: Warp **v0.2026.09.16.08.27**; docs pages
"Last updated Sep 24, 2026". Feed: page <https://docs.warp.dev/changelog>
(yearly sub-page `/changelog/2026/`); cursor was `—`, newest entry
**2026.09.16** recorded. Relevant entries 2026-07-17 → 2026-09-27: file-backed
execution profiles to stable (2026.07.31), a built-in Factory MCP server for
logged-in users (2026.08.18).

## Claims

Researcher (sonnet) table, verbatim. Numbered rows re-verify ledger claims;
`I` rows are changelog or surface differences found in the sweep.

| # | Claim | Ledger location | Old value | New value | Source URL | Verbatim quote | Vendor version |
|---|---|---|---|---|---|---|---|
| 1 | "Rules on disk" for Warp: declined, global rules UI/cloud-managed, no on-disk path at all | `.claude/rules/vendor-capability-watchlist.md:184` | declined — "global rules are UI/cloud-managed with **no on-disk path at all** — nothing for grim to own" | **changed** — Warp now ships a filesystem Project Rules surface: `AGENTS.md` (or `WARP.md`) at repo root and in subdirectories. "Action when shipped" condition ("enable if Warp ships a filesystem rules surface") is now met for Project Rules; Global Rules remain UI/cloud-managed (Warp Drive) | https://docs.warp.dev/knowledge-and-collaboration/rules | "Project Rules live in your codebase and apply automatically when working within that project. They're stored in an `AGENTS.md` file (or `WARP.md` for backwards compatibility)." | docs last updated Sep 24, 2026 |
| 2 | "Rule + Agent kinds" for Warp (part of multi-vendor row): declined — monolithic instruction files, no in-file scoping key | `.claude/rules/vendor-capability-watchlist.md:183` | declined — no per-file scoping key, same class as Droid's hierarchical `AGENTS.md` | unchanged for the Rule kind — Warp's `AGENTS.md`/`WARP.md` apply by directory location only, matching the already-called-out Droid-style pattern; no documented per-file frontmatter scoping key (`paths:`/`globs:`) found. Action-when-shipped condition ("a documented per-file scoping key") still not met | https://docs.warp.dev/knowledge-and-collaboration/rules | "Warp automatically applies the `AGENTS.md` (or `WARP.md`) in the root and in the current directory." (no scoping-key mechanism described) | docs last updated Sep 24, 2026 |
| 3 | `src/install/vendor_warp.rs` module doc: "**MCP**: declined. No grim-writable config file surface." | `src/install/vendor_warp.rs` (module doc, MCP bullet) | declined — "No grim-writable config file surface" | **changed** — Warp now reads/writes on-disk MCP server config: `~/.warp/.mcp.json` (global) and `{repo_root}/.warp/.mcp.json` (project-scoped), JSON format, and even offers a built-in `/agent-add-mcp` skill that writes these files | https://docs.warp.dev/agents/capabilities/mcp/ | "The built-in `/agent-add-mcp` skill lets the Warp Agent create or update file-based MCP server definitions. Choose whether to save globally or in the current project — the skill writes the server definition to the matching file: **Global:** `~/.warp/.mcp.json` **Project-scoped:** `{repo_root}/.warp/.mcp.json`" | docs last updated Sep 24, 2026 |
| 4 | `src/install/vendor_warp.rs`: skills at `.warp/skills/<name>/` (project) and `~/.warp/skills/<name>/` (global) | `src/install/vendor_warp.rs` (module doc, Skills bullet) | `.warp/skills/<name>/` project, `~/.warp/skills/<name>/` global | unchanged — both paths confirmed current | https://docs.warp.dev/agents/capabilities/skills/ | "Each skill requires its own subdirectory containing a `SKILL.md` file." (listed under both `.warp/skills/` and `~/.warp/skills/` in the project/global directory lists) | docs last updated Sep 24, 2026 |
| 5 | `src/install/vendor_warp.rs`: pool-capable — Warp scans `.agents/skills` at both scopes, first-party confirmed | `src/install/vendor_warp.rs` (module doc, Pool-capable bullet) | `.agents/skills` scanned at project + global scope | unchanged — confirmed, and now documented as the **recommended** location (Warp's own preference, ahead of `.warp/skills/` in the doc's list order) | https://docs.warp.dev/agents/capabilities/skills/ | Project list includes "`.agents/skills/` (recommended)" and "`.warp/skills/`"; Global list includes "`~/.agents/skills/` (recommended)" and "`~/.warp/skills/`" | docs last updated Sep 24, 2026 |
| 6 | `src/install/vendor_warp.rs`: "**No environment override found.**" (for `~/.warp/` skills/detection root) | `src/install/vendor_warp.rs` (module doc, closing paragraph) | no env override found | **changed for skills, cloud-agent-scoped** — Warp now documents `WARP_SKILL_DIRS`, an env var that adds extra skill-index directories for cloud agent runs (not a relocation of `~/.warp/` itself, and not documented as applying to local desktop/CLI detection) | https://docs.warp.dev/agents/capabilities/skills/ | "To index skills that live outside those repositories — for example, skills baked into a custom Docker image — set the `WARP_SKILL_DIRS` environment variable." | docs last updated Sep 24, 2026 |
| 7 | `src/install/vendor_warp.rs`: "**Agents**: declined. Agent profiles are Settings-UI-only." | `src/install/vendor_warp.rs` (module doc, Agents bullet) | declined — Settings-UI-only | **partially changed** — as of 2026-07-31 (PR warpdotdev/warp#14418, "Promote FileBackedExecutionProfiles to Stable"), agent execution profiles are file-backed for all users: stored in the user-editable `settings.toml` under `[agents.profiles]`, not only the Settings UI. No separate per-agent installable file exists (still one shared settings file, not grim's per-artifact model) | https://docs.warp.dev/terminal/settings/ ; https://github.com/warpdotdev/warp/pull/14418 | "You can edit it directly in any text editor, check it into version control, or generate it with a script." — schema example shows `[agents.profiles]` with keys such as `agent_mode_coding_permissions = "always_allow_reading"` | PR #14418 merged into changelog entry 2026.07.31 (v0.2026.07.29.09.05); docs last updated Sep 24, 2026 |

Unsourced:
- Whether Warp's `~/.warp/skills/` global root is truly identical across macOS/Linux/Windows (as `src/install/vendor_warp.rs` claims) — the skills doc lists `~/.warp/skills/` without an OS breakdown, but the separate settings.toml doc shows the *app settings file itself* is NOT at a uniform `~/.warp/`-relative path across OSes (macOS: `~/.warp/settings.toml`; Linux: `~/.config/warp-terminal/settings.toml`; Windows: `%LOCALAPPDATA%\warp\Warp\config\settings.toml`). Could not find a page stating explicitly whether the skills/MCP data root follows the settings-file convention or the claimed uniform `~/.warp/` convention on Linux/Windows specifically — flagging as a gap, not asserting a change, since no primary source directly contradicts the skills-root claim.

Feed notes:
- Newest tag or heading seen: changelog entry **2026.09.16** (v0.2026.09.16.08.27), on https://docs.warp.dev/changelog/2026/. Cursor found: n/a — cursor was `—`.
- Vendor version current today: v0.2026.09.16.08.27 (latest changelog entry, https://docs.warp.dev/changelog/2026/; page itself "Last updated Sep 24, 2026").
- Anything grim renders or documents that the feed shows changed but no ledger row covers:
  - Warp now bundles a **built-in Factory MCP server for logged-in users** (changelog 2026.08.18, v0.2026.08.18.02.52) — not something grim writes, but changes what "MCP" means inside Warp; no ledger row references it. https://docs.warp.dev/changelog/2026/
  - Warp's skills docs now list eight additional third-party skill directories it natively scans besides `.warp/skills/` and `.agents/skills/` — `.claude/skills/`, `.codex/skills/`, `.cursor/skills/`, `.gemini/skills/`, `.copilot/skills/`, `.factory/skills/`, `.github/skills/`, `.opencode/skills/` (both project and global scope). `src/install/vendor_warp.rs` and the watchlist only mention `.warp/skills/` and the `.agents/skills/` pool; no row covers this broader native-scan list. https://docs.warp.dev/agents/capabilities/skills/

## Verifier sign-off

One opus verifier (C-008) re-fetched each claim that drives a docs, comment,
watchlist-wording or issue-draft change. No claim in this pass drives a
renderer, metadata or validation change unless Routing says so.

| Claim | Verdict | Evidence URL | Verbatim quote / note |
|---|---|---|---|
| V20 | CONFIRMED | https://docs.warp.dev/agents/capabilities/mcp/ ; https://docs.warp.dev/knowledge-and-collaboration/rules | File-based MCP "Global: `~/.warp/.mcp.json`", "Project-scoped: `{repo_root}/.warp/.mcp.json`" (with approval gates). Rules: "They're stored in an `AGENTS.md` file (or `WARP.md` for backwards compatibility)"; scoping is directory-based only, no frontmatter/glob key documented. |

## Live CLI check

n/a — `sweep` depth on a Tier 2 harness (the live check is `deep` only).

## Routing

| Claim | Class | Landed as |
|---|---|---|
| 3 / V20 file MCP `~/.warp/.mcp.json` | (a) + (c) | module doc; new `MCP kind` row (Cline, Droid, Warp); issue draft 1 |
| 1, 2 / V20 project rules `AGENTS.md`/`WARP.md` | (a) | `Rules on disk` row: on disk now, but location-scoped with no key — no action |
| 7 agent profiles in `settings.toml` | (a) | module doc: a shared settings file, not per-agent files |
| 4, 5 | (a) | confirmed unchanged (`.agents/skills` is Warp's "recommended" location); stamp dated |
| 6 `WARP_SKILL_DIRS` | finding | cloud-agent skill index only; relocates nothing grim writes |
| extra skill dirs (`.claude/skills`, `.codex/skills`, … 8 more) | finding | Warp reads them; grim writes `.warp/skills` or the pool, so nothing moves |

Security: none found.

## Issue drafts

Filed as https://github.com/grimoire-rs/grimoire/issues/155.

### 1. Warp: enable MCP for `~/.warp/.mcp.json` and `.warp/.mcp.json`?

**Labels:** `enhancement`, `vendor:warp`, `needs-design`

Warp documents file-based MCP servers: "Global: `~/.warp/.mcp.json`",
"Project-scoped: `{repo_root}/.warp/.mcp.json`"
(<https://docs.warp.dev/agents/capabilities/mcp/>, verified 2026-09-27,
v0.2026.09.16). grim declines MCP for Warp ("no grim-writable config file").
Before enabling: confirm the JSON shape (`mcpServers`?), the env-ref form and
the approval gate Warp puts on project-scoped servers. A kind enablement
changes the grid, the ADR mapping table and the docs matrix, and needs an
upgrade note.

## Friction

- Warp's settings file lives at an OS-specific path (macOS `~/.warp/settings.toml`, Linux `~/.config/warp-terminal/`, Windows `%LOCALAPPDATA%`); no page says whether the skills root follows it. The module doc's "same on every OS" claim is unrefuted but unconfirmed this pass.
- Researcher sandboxes denied network `curl` (and, for some, `gh`); the brief's "`curl -sL <url>` is fine" tip was wrong there. Researchers fell back to WebFetch, whose text is model-extracted, so quotes from vendor sites are summarizer-sourced; GitHub sources were read raw via `gh api` where allowed. Brief fixed.
- Driver cost: the brief told researchers to return only the table, so a 14-target run would have re-typed every table. This run gave each researcher one scratchpad output file and assembled the artifacts from it. Brief now names that file.

## Skill changes

One `chore(skills):` commit for the whole WP-G run, [f9effcc1](https://github.com/grimoire-rs/grimoire/commit/f9effcc1): researcher-brief scratchpad output file, no-curl fallback and re-fetch warning; domains.md docs and feed URLs (kiro, junie, amp, antigravity, goose, openclaw, qoder), `JUNIE_HOME` / `OPENCLAW_HOME` cells, section-default and stamp re-dating note; SKILL.md one artifact per target in a `--domain` run.
