# Research: upstream refresh — amp (sweep)

## Metadata
**Date:** 2026-11-16 · **Domain:** vendors · **Depth:** sweep — checked 50d ago
**Triggered by:** /upstream-refresh
**Expires:** 2027-05-16

## Claims

| # | Claim | Ledger location | Old value | New value | Source URL | Verbatim quote | Vendor version |
|---|---|---|---|---|---|---|---|
| 1 | Amp reads `~/.agents/skills` (pool writer for auto-join) | .agents/upstream-watchlist.md:17 | Amp is a client that writes/reads the shared pool | unchanged | https://ampcode.com/docs/markdown/customize/skills | "2. `~/.agents/skills/`" | none stated (page lastModified 2026-10-21) |
| 2 | `$AMP_SETTINGS_FILE` not in docs; only `--settings-file` documented (docs half only) | .agents/upstream-watchlist.md:83; vendor_amp.rs:33-60; vendor-metadata.md:494 | not honored; docs name only `--settings-file` | unchanged | https://ampcode.com/docs/markdown/cli/settings | "Custom user settings: pass `--settings-file <path>` to point Amp at a different user settings file" | none (lastModified 2026-11-02, same as ledger) |
| 3 | No XDG mention; `~/.config/amp/` on macOS and Linux, `%USERPROFILE%\.config\amp\` on Windows | .agents/upstream-watchlist.md:84; vendor_amp.rs:58-75; settings.md:407 | UNRESOLVED, zero XDG mentions | unchanged (grep of settings and skills pages for "xdg": 0 hits) | https://ampcode.com/docs/markdown/cli/settings | "Linux: `~/.config/amp/settings.json` or `~/.config/amp/settings.jsonc`" | none |
| 4 | Skills scan order, top three `~/.config/agents/skills` > `~/.agents/skills` > `~/.config/amp/skills` | .agents/upstream-watchlist.md:86; vendor_amp.rs:8-26 | same top three, then project `.agents/skills`, `.claude/skills`, ... | unchanged (list is now 11 entries: adds `amp.skills.path` dirs, personal and workspace repos, official skills; also local dirs searched recursively up to five levels) | https://ampcode.com/docs/markdown/customize/skills | "Amp uses the first skill with a given frontmatter `name`. The order is: 1. `~/.config/agents/skills/` 2. `~/.agents/skills/` 3. `~/.config/amp/skills/`" | none |
| 5 | `amp.skills.disableGlobalAgentsSkills` skips the two global agents dirs | .agents/upstream-watchlist.md:86 | opt-out skips `~/.config/agents/skills/` and `~/.agents/skills/` | unchanged | https://ampcode.com/docs/markdown/cli/settings | "Disable loading skills from the global agent-agnostic directories (`~/.config/agents/skills/` and `~/.agents/skills/`)." | none |
| 6 | Hook kind declined: Amp's only hook surface is a JS/TS plugin module | .agents/upstream-watchlist.md:236 (Amp), :255 (OpenCode, same shape) | plugin module, grim would generate code | unchanged | https://ampcode.com/docs/markdown/customize/plugins | "Plugins are TypeScript or JavaScript modules that add tools, commands, and event-driven behavior to Amp." and "**Handle events** — `amp.on(...)` for tool calls, tool results, and agent lifecycle events" | none (lastModified 2026-11-14) |
| 7 | Settings key list row: 8 grim rows, global only; re-check trigger (model/mode/marketplace key, or merge for `amp.tools.disable`) not fired | .agents/upstream-watchlist.md:302 | 8 rows; page lastModified 2026-11-02 | unchanged. Page lastModified still 2026-11-02. Page lists further keys grim does not carry (additive candidates, none is a model/mode/marketplace key): `amp.fuzzy.alwaysIncludePaths`, `amp.keymap`, `amp.remoteThreadCreation.enabled`, `amp.runner.autoUpdate.enabled`, `amp.runner.env.enabled`, `amp.skills.disableClaudeCodeSkills`, `amp.skills.disableGlobalAgentsSkills`, `amp.skills.path`, `amp.tools.disable`, `amp.mcpPermissions`; `amp.tools.disable` has no stated merge | https://ampcode.com/docs/markdown/cli/settings | "Disable specific tools by name. Use `builtin:toolname` to disable only the built-in tool with that name while allowing an MCP server to provide a tool by the same name." | none |
| 8 | `settings.json` vs `.jsonc` precedence unstated; workspace overrides user | .agents/upstream-watchlist.md:310; settings.md:990-992 | page names both file names, never says which wins | unchanged. Workspace file is now described as "the nearest `.amp/settings.json` or `.amp/settings.jsonc`, searched upward" to the repo root | https://ampcode.com/docs/markdown/cli/settings | "When the same setting appears in multiple places, workspace settings override user settings." | none |
| 9 | `/amp.showCosts` bool, scalar, global | vendor_amp.rs:99; settings.md:754 | Bool | unchanged | https://ampcode.com/docs/markdown/cli/settings | "**`amp.showCosts`** **Type:** `boolean`, **Default:** `true`" | none |
| 10 | `/amp.notifications.enabled` bool | vendor_amp.rs:100; settings.md:755 | Bool | unchanged | https://ampcode.com/docs/markdown/cli/settings | "**`amp.notifications.enabled`** **Type:** `boolean`, **Default:** `true`" | none |
| 11 | `/amp.terminal.copyOnSelect` bool | vendor_amp.rs:101; settings.md:756 | Bool | unchanged | https://ampcode.com/docs/markdown/cli/settings | "**`amp.terminal.copyOnSelect`** **Type:** `boolean`, **Default:** `true`" | none |
| 12 | `/amp.terminal.detailsExpandedByDefault` bool | vendor_amp.rs:102; settings.md:757 | Bool | unchanged | https://ampcode.com/docs/markdown/cli/settings | "**`amp.terminal.detailsExpandedByDefault`** **Type:** `boolean`, **Default:** `false`" | none |
| 13 | `/amp.thread.autoArchiveOnQuit` bool | vendor_amp.rs:103; settings.md:758 | Bool | unchanged | https://ampcode.com/docs/markdown/cli/settings | "**`amp.thread.autoArchiveOnQuit`** **Type:** `boolean`, **Default:** `false`" | none |
| 14 | `/amp.updates.mode` enum auto/warn/disabled | vendor_amp.rs:104-109; settings.md:759 | Enum auto, warn, disabled | unchanged (docs type is `string`; the three values are in prose) | https://ampcode.com/docs/markdown/cli/settings | "`\"warn\"` shows update notifications. `\"disabled\"` turns off update checks. `\"auto\"` installs updates automatically." | none |
| 15 | `/amp.git.commit.ampThread.enabled` bool | vendor_amp.rs:110; settings.md:760 | Bool | unchanged | https://ampcode.com/docs/markdown/cli/settings | "**`amp.git.commit.ampThread.enabled`** **Type:** `boolean`, **Default:** `true`" | none |
| 16 | `/amp.git.commit.coauthor.enabled` bool | vendor_amp.rs:111; settings.md:761 | Bool | unchanged | https://ampcode.com/docs/markdown/cli/settings | "**`amp.git.commit.coauthor.enabled`** **Type:** `boolean`, **Default:** `true`" | none |
| 17 | Settings are global-only for grim (scalar merge); Amp has a workspace tier that wins | vendor_amp.rs:98-111; settings.md:990 | GlobalOnly | unchanged (grim's choice; upstream tier exists) | https://ampcode.com/docs/markdown/cli/settings | "Workspace settings: the nearest `.amp/settings.json` or `.amp/settings.jsonc`, searched upward from your current working directory to the repository root" | none |
| 18 | MCP: literal key `amp.mcpServers`, workspace `.amp/settings.json`, global `~/.config/amp/settings.json`, `${VAR_NAME}` env refs, `command`/`args`/`env` | vendor_amp.rs:29-33; mcp-servers.md:227 | as left | unchanged | https://ampcode.com/docs/markdown/customize/mcp | "In configuration files, set `amp.mcpServers` and use `${VAR_NAME}` syntax for environment variables" | none (lastModified 2026-11-04) |
| 19 | MCP oauth skipped by grim | mcp-servers.md:227 | `oauth skipped` | unchanged (Amp does local OAuth for URL entries; grim skipping stays valid) | https://ampcode.com/docs/markdown/customize/mcp | "When you start the Amp TUI after adding a URL entry to `amp.mcpServers`, Amp opens the" (cut at line end in source) | none |
| 20 | Expansion in config strings: new `${amp:id-token}` in MCP headers (not previously recorded) | vendor_amp.rs:29-33; mcp-servers.md:227 | only `${VAR_NAME}` passthrough recorded | NEW: `${amp:id-token}` expands in MCP `headers` of HTTPS servers without OAuth, orb only; rejected in URLs, command, args, env | https://ampcode.com/docs/markdown/customize/mcp | "`${amp:id-token}` works only in headers of HTTPS servers without OAuth configuration. Amp rejects it in URLs and in the command, arguments, and environment of local servers." | none (lastModified 2026-11-04) |
| 21 | Trust gates: no proposed row pre-trusts; workspace MCP needs approval | vendor_amp.rs settings rows; mcp-servers.md:227 | none proposed | unchanged for the 8 rows. Docs name `amp.mcpTrustedServers` (a pre-trust key, local to a machine), not proposed as a row | https://ampcode.com/docs/markdown/cli/settings | "`amp.mcpTrustedServers` is local to one machine." and (mcp page) "MCP servers in workspace settings (`.amp/settings.json`) require explicit approval before they can run." | none |
| 22 | Rules declined: "only instruction surface is AGENTS.md ... no per-file scoping" | clients.md:501-502; vendor_amp.rs:36-37 | no per-file scoping | CHANGED: docs now describe `globs` YAML frontmatter on @-mentioned files, scoped injection. Fallback `AGENT.md` then `CLAUDE.md` unchanged | https://ampcode.com/docs/markdown/customize/agents-md | "To provide guidance that only applies when working with certain files, you can specify `globs` in YAML front matter of mentioned files." and "If no `AGENTS.md` exists in a directory, but a file named `AGENT.md` (without an `S`) or `CLAUDE.md` does exist, that file will be included." | none (lastModified 2026-11-13) |
| 23 | Global AGENTS.md locations | not in ledger (context for #22) | — | `$HOME/.config/amp/AGENTS.md` and `$HOME/.config/AGENTS.md` always included | https://ampcode.com/docs/markdown/customize/agents-md | "System-wide guidance files, as well as both `$HOME/.config/amp/AGENTS.md` and `$HOME/.config/AGENTS.md`, are always included if they exist." | none |
| 24 | Skill `SKILL.md` frontmatter keys name/description; dir name must match name | vendor-metadata.md:436 (Amp carries no vendor namespace) | no namespace | unchanged (docs show only `name`, `description`; a skill may add `mcpServers` in frontmatter) | https://ampcode.com/docs/markdown/customize/skills | "A skill can define MCP servers in a sibling `mcp.json` file or in the `mcpServers` field of its `SKILL.md` frontmatter." | none |

## Verifier sign-off

Signed off 2026-11-16 by three read-only sonnet verifiers (upstream re-fetched, grim code read) and by the implementing workers; one line per code-driving claim.

- `${amp:id-token}` finding: refuted, NOT REAL; the token passes through as Amp intends (driver verification 2026-11-16). No code landed for Amp.

## Routing

- #3 No XDG mention; ~/.config/amp/ on macOS and Linux, %USERPROFILE%\.config\amp\... → (a) ledger or doc text fix
- #4 Skills scan order, top three ~/.config/agents/skills > ~/.agents/skills >... → (a) ledger or doc text fix
- #7 Settings key list row: 8 grim rows, global only; re-check trigger... → (a) ledger or doc text fix
- #8 settings.json vs .jsonc precedence unstated; workspace overrides user → (a) ledger or doc text fix
- #14 /amp.updates.mode enum auto/warn/disabled → (a) ledger or doc text fix
- #17 Settings are global-only for grim (scalar merge); Amp has a workspace tier that... → (a) ledger or doc text fix
- #19 MCP oauth skipped by grim → (a) ledger or doc text fix
- #22 Rules declined: "only instruction surface is AGENTS.md ... no per-file scoping" → (c) issue draft
- #23 Global AGENTS.md locations → (c) issue draft
- #24 Skill SKILL.md frontmatter keys name/description; dir name must match name → (a) ledger or doc text fix
- (c) security — docs/src/content/docs/mcp-servers.md:227 · expansion
- (c) security — docs/src/content/docs/mcp-servers.md:227 · trust-gate key (not a row)
- `${amp:id-token}` in MCP values → dropped: not real, passes through as Amp intends

## Issue drafts

### Amp rules: globs frontmatter scoping and global AGENTS.md locations

Amp's AGENTS.md page now describes `globs` YAML frontmatter on @-mentioned files for scoped injection, and always includes `$HOME/.config/amp/AGENTS.md` and `$HOME/.config/AGENTS.md`. Source: https://ampcode.com/docs/markdown/customize/agents-md. grim declines rules for Amp on the premise of no per-file scoping (`docs/src/content/docs/clients.md:501-502`, `src/install/vendor_amp.rs:36-37`).

Labels: enhancement

## Friction

- dropped (unsourced): `AMP_SETTINGS_FILE` existence in the shipped binary (watchlist:83, binary `0.0.1793887133-gbf8b08`) — did not download or read the npm binary (read-only, install nothing); only the docs half is re-verified (row 2).
- dropped (unsourced): clients.md:502 "Amp subagents are spawned at runtime with no installable file format" — `/docs/markdown/customize/subagents` returned 404; no other subagent page located.
- dropped (unsourced): vendor-metadata.md:436 "Amp carries no vendor namespace" beyond skills page frontmatter keys — no frontmatter reference page found (row 24 covers only what the skills page shows).
- feed: newest "Boxes" (10 Nov 2026), cursor found yes

## Skill changes

Host caps concurrent subagents at 20; the researcher built its own claim list; replies are a short summary with the table in a file. Fixed in this run's chore(skills) commit.
