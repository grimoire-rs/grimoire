# Research: upstream refresh — warp (sweep)

## Metadata
**Date:** 2026-11-16 · **Domain:** vendors · **Depth:** sweep — checked 50d ago
**Triggered by:** /upstream-refresh
**Expires:** 2027-05-16

## Claims

| # | Claim | Ledger location | Old value | New value | Source URL | Verbatim quote | Vendor version |
|---|---|---|---|---|---|---|---|
| 1 | MCP files: project `.warp/.mcp.json`, global `~/.warp/.mcp.json`, key `mcpServers` | .agents/upstream-watchlist.md:113; src/install/vendor_warp.rs:30 | as stated | unchanged | https://docs.warp.dev/agents/capabilities/mcp.md | "Global: `~/.warp/.mcp.json`" / "Project-scoped: `{repo_root}/.warp/.mcp.json`"; "Each entry under `mcpServers` is keyed by a unique name" | docs live 2026-11-16; changelog newest v0.2026.11.11.08.17 |
| 2 | stdio fields `command`/`args`/`env`/`working_directory`; remote `url`/`headers` | watchlist:113 | as stated | unchanged | https://docs.warp.dev/agents/capabilities/mcp.md | "`working_directory` | string | No | Working directory path where the command is run" ; "`headers` | object | No | Key-value object of header variables (e.g., Authorization)." | same |
| 3 | Project-scoped servers need approval, never auto-spawn | watchlist:113 | as stated | unchanged | https://docs.warp.dev/agents/capabilities/mcp.md | "Project-scoped servers never auto-spawn - Warp detects project-scoped MCP config files in cloned repos, but requires you to start each server manually." | same |
| 4 | Warp also reads Claude's `.mcp.json` / `~/.claude.json` and `.agents/.mcp.json`; third-party global servers gated by "Auto-spawn servers from third-party agents" | watchlist:113; docs mcp-servers.md:246-249; clients.md:643-646 | as stated | unchanged | https://docs.warp.dev/agents/capabilities/mcp.md | "Global servers from all other providers auto-spawn only when **Auto-spawn servers from third-party agents** is enabled. Project-scoped servers from any provider require explicit approval." | same |
| 5 | OAuth is a browser login with no config keys | watchlist:113; clients.md:635 | as stated | unchanged | https://docs.warp.dev/agents/capabilities/mcp.md | "**OAuth login (one-click installation)**: simplifies configuration by handling authentication through your browser." | same |
| 6 | No sentence says Warp expands `${VAR}` in the MCP file (example carries `${X_CHROMA_TOKEN}` in args) | watchlist:113; clients.md:637 | as stated | unchanged | https://docs.warp.dev/agents/capabilities/mcp.md | "\"args\": [\"mcp-remote\", \"https://mcp.trychroma.com/package-search/v1\", \"--header\", \"x-chroma-token: ${X_CHROMA_TOKEN}\"]" | same |
| 7 | Remote transports documented are streamable HTTP and SSE only (ws skipped) | watchlist:113; mcp-servers.md:229 | ws undocumented | unchanged | https://docs.warp.dev/agents/capabilities/mcp.md | "Streamable HTTP or SSE Server (URL) — remote or locally hosted MCP endpoints." | same |
| 8 | Skills: `.warp/skills/` and `~/.warp/skills/` are first-class scanned dirs; `.agents/skills` scanned at both scopes (pool-capable) | vendor_warp.rs:6-20; vendor-metadata.md:470,499; clients.md:600-603 | as stated | unchanged | https://docs.warp.dev/agents/capabilities/skills.md | "**`.agents/skills/`** (recommended)" ... "**`.warp/skills/`**"; "Use the same supported directories under your home directory, such as `~/.agents/skills/` (recommended) or `~/.warp/skills/`." (ten project dirs listed, same count as grim's comment) | same |
| 9 | `~/.warp/` MCP config and skills are the same path on macOS, Linux and Windows | clients.md:603; vendor-metadata.md:499 | as stated | unchanged | https://docs.warp.dev/terminal/settings/file-locations.md | "a small set of cross-cutting files always live in your home directory on every platform — currently MCP server config and bundled skills." (Linux: "**MCP server config** - `~/.warp/.mcp.json`") | same |
| 10 | Rules: Project Rules on disk in `AGENTS.md`/`WARP.md`, by directory, no scoping key; Global Rules not on disk | watchlist:120,123; vendor_warp.rs:16 | declined | unchanged (rules decline stands) | https://docs.warp.dev/agents/capabilities/rules.md | "They’re stored in an `AGENTS.md` file (or `WARP.md` for backwards compatibility)"; "Warp automatically applies the `AGENTS.md` (or `WARP.md`) in the root and in the current directory." | same |
| 11 | Rules page URL moved: ledger cites `https://docs.warp.dev/knowledge-and-collaboration/rules` | watchlist:123 | knowledge-and-collaboration/rules | https://docs.warp.dev/agents/capabilities/rules/ (HTTP 308 redirect from old URL) | https://docs.warp.dev/knowledge-and-collaboration/rules | HTTP 308, `location: /agents/capabilities/rules/` | same |
| 12 | Agents declined: agent profiles live in the settings file, not per-agent files | watchlist:120; vendor_warp.rs:18 | as stated | unchanged | https://docs.warp.dev/terminal/settings/all-settings.md | "`execution_profiles` — The collection of agent profiles and their permissions, shared by Agent Mode and the terminal agent. This is a structured object that Warp manages when you create and edit profiles" | same |
| 13 | Settings key list: 29 rows, all exist upstream with grim's type (28 bool/enum matched by key name and Type line; `/agents/warp_agent/active_ai/enabled` present under `[agents.warp_agent.active_ai]`) | watchlist:307; vendor_warp.rs:70-225 | 29 rows | unchanged | https://docs.warp.dev/terminal/settings/all-settings.md | "`thinking_display_mode` — ... Type: string. Default: `\"show_and_collapse\"`. Options: `\"show_and_collapse\"`, `\"always_show\"`, `\"never_show\"`."; "`file_based_mcp_enabled` — ... Type: boolean. Default: `false`." | same |
| 14 | Enum values: `default_prompt_submission_mode` documented `interrupt`,`queue` (schema-only `steer` left out) | vendor_warp.rs:92-100 | interrupt, queue | unchanged | https://docs.warp.dev/terminal/settings/all-settings.md | "Options: `\"interrupt\"`, `\"queue\"`." | same |
| 15 | `/general/default_session_mode` options; `tab_config` left out (needs companion path key) | vendor_warp.rs:102-109 | terminal, agent, cloud_agent, docker_sandbox | unchanged | https://docs.warp.dev/terminal/settings/all-settings.md | "`default_session_mode` — ... Options: `\"terminal\"`, `\"agent\"`, `\"cloud_agent\"`, `\"tab_config\"`, `\"docker_sandbox\"`." and "`default_tab_config_path` — Path to the tab config used when `default_session_mode` is `\"tab_config\"`." | same |
| 16 | `[agents.profiles]` and `execution_profiles` still both on the all-settings page (re-check trigger not fired; PD-21 stays) | watchlist:307; domains.md:107 | disagree | unchanged | https://docs.warp.dev/terminal/settings/all-settings.md | "**Section**: `[agents.profiles]`" and "`execution_profiles` — ... edit it through the UI rather than by hand." | same |
| 17 | Settings file: `settings.toml`, `~/.warp` (macOS), `${XDG_CONFIG_HOME:-$HOME/.config}/warp-terminal` (Linux), TOML v1.1 | vendor_warp.rs:34-40; settings.md:413; domains.md:107 | as stated | unchanged | https://docs.warp.dev/terminal/settings/file-locations.md | "**Settings file** - `${XDG_CONFIG_HOME:-$HOME/.config}/warp-terminal/settings.toml`"; settings page: "The file uses [TOML v1.1](https://toml.io/en/v1.1.0) syntax." | same |
| 18 | Live reload and other writers (Settings panel, `modify-settings` skill) | watchlist:324; vendor_warp.rs:41 | as stated | unchanged | https://docs.warp.dev/terminal/settings.md | "**Hot-reload** — Warp watches `settings.toml` for changes and applies them instantly when you save the file."; "The bundled `modify-settings` skill handles the file update automatically." | same |
| 19 | Unknown value falls back to defaults | watchlist:325 | as stated | unchanged | https://docs.warp.dev/terminal/settings.md | "If the file contains invalid TOML or an unrecognized value, Warp shows a warning banner and falls back to defaults for the affected settings." | same |
| 20 | Preview builds use separate dirs, not covered | watchlist:326; domains.md:107 | warp-terminal-preview / ~/.warp-preview | unchanged | https://docs.warp.dev/terminal/settings.md | "Preview — `~/.warp-preview/settings.toml`"; "Preview — `~/.config/warp-terminal-preview/settings.toml`" | same |
| 21 | No Windows root: settings under `%LOCALAPPDATA%\warp\Warp\config` | watchlist:327; clients.md:283; settings.md:413 | as stated | unchanged | https://docs.warp.dev/terminal/settings/file-locations.md | "**Settings file** - `%LOCALAPPDATA%\\warp\\Warp\\config\\settings.toml`" | same |

## Verifier sign-off

none required — no code landed in this run.

## Routing

- #10 Rules: Project Rules on disk in AGENTS.md/WARP.md, by directory, no scoping... → (a) ledger or doc text fix
- #11 Rules page URL moved: ledger cites... → (a) ledger or doc text fix

## Issue drafts

none

## Friction

- dropped (unsourced): Hook kind "Warp documents none" (watchlist:237) — absence claim. Tried: `docs.warp.dev/agents/capabilities/hooks.md` and `/subagents.md` both 404; llms.txt is a 2.5 KB stub with no index; changelog 2026.09.23-2026.11.12 has no hook entry (only a Claude Code StopFailure fix in an older entry). Suggests unchanged, but no positive quote exists.
- dropped (unsourced): Expansion claim (does Warp expand variables or file refs in a settings.toml string) — no statement on terminal/settings.md or all-settings.md (grep for expand/$/environment variable: nothing); absence only.
- dropped (unsourced): Trust-gate claim (no key that disables a trust gate or pre-trusts a folder) — grep for trust/folder/safe over all-settings.md found only `auto_approve_bypasses_command_denylist` (already a grim row, a command-denylist bypass, not a workspace trust gate). Absence only.
- dropped (unsourced): Global Rules "UI/cloud-managed" (vendor_warp.rs:16) — rules page says only "Global Rules apply across all projects and contexts" with no storage location.
- not reached: vendor-metadata.md:499 "None found in current docs" (legacy skill path column); clients.md:556-574 matrix prose beyond the rows above; docs/settings.md:868-880 per-key table (covered through row 13 against the code list instead).
- feed: newest 2026.11.12, cursor found yes

## Skill changes

Host caps concurrent subagents at 20; the researcher built its own claim list; replies are a short summary with the table in a file. Fixed in this run's chore(skills) commit.
