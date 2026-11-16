# Research: upstream refresh — cursor (sweep)

## Metadata
**Date:** 2026-11-16 · **Domain:** vendors · **Depth:** sweep — checked 50d ago
**Triggered by:** /upstream-refresh
**Expires:** 2027-05-16

## Claims

| # | Claim | Ledger location | Old value | New value | Source URL | Verbatim quote | Vendor version |
|---|---|---|---|---|---|---|---|
| 1 | `/permissions/allow`, `/permissions/deny`: string[], required, Set, GlobalOnly | src/install/vendor_cursor.rs:76,80 | string list, required | unchanged | https://cursor.com/docs/cli/reference/configuration | "permissions.allow string[] Permitted operations" / "permissions.deny string[] Forbidden operations" (under "Required fields") | CLI docs 2026-11-16 (changelog newest v2026.10.22) |
| 2 | `/editor/vimMode` bool, required | vendor_cursor.rs:96 | bool, required | unchanged | same | "editor.vimMode boolean Enable Vim keybindings (default: false)" (under "Required fields") | same |
| 3 | `/approvalMode` enum allowlist/auto-review/unrestricted | vendor_cursor.rs:83 | 3 literals | unchanged | same | "approvalMode string Approval mode: allowlist , auto-review , or unrestricted" | same |
| 4 | `/sandbox/readBoundary` enum system/workspace | vendor_cursor.rs:89 | system, workspace | unchanged | same | "sandbox.readBoundary string Read access mode: system (default) or workspace" | same |
| 5 | `/notifications`, `/hints`, `/rewind`, `/suggestNextPrompt`, `/maxMode` bool scalar | vendor_cursor.rs:98-102 | bool | unchanged | same | "notifications boolean Send a terminal notification when the agent finishes or needs input"; "hints boolean ..."; "rewind boolean Enable /rewind ..."; "suggestNextPrompt boolean ..."; "maxMode boolean Persisted preference for max mode in the model picker" | same |
| 6 | `/display/showLineNumbers`, `showThinkingBlocks`, `showStatusIndicators`, `showStatusLineRunningTime` bool | vendor_cursor.rs:103-106 | bool | unchanged | same | "display.showLineNumbers boolean Show line numbers in rendered code blocks"; "display.showStatusLineRunningTime boolean Show elapsed running time in the status line" (all four listed, boolean) | same |
| 7 | `/attribution/attributeCommitsToAgent`, `attributePRsToAgent` bool | vendor_cursor.rs:107-108 | bool | unchanged | same | "attribution.attributeCommitsToAgent boolean Add "Made with Cursor" trailer to Agent commits (default: true)" | same |
| 8 | Settings key list: reference lists keys grim has no row for | upstream-watchlist.md:296 (adjacent), vendor_cursor.rs:73 | not on list | new documented keys: `channel` (string), `sandbox.mode` (string, no values), `sandbox.networkAccess` (string, no values), `network.useHttp1ForAgent` (boolean, default false). `autoAcceptWebSearch` still absent. Additive candidate only: `/network/useHttp1ForAgent` is a typed bool; the two sandbox keys have no documented value set | https://cursor.com/docs/cli/reference/configuration | "network.useHttp1ForAgent boolean Use HTTP/1.1 instead of HTTP/2 for agent connections (default: false )"; "sandbox.mode string Sandbox mode override" | same |
| 9 | Settings file path order (no order stated) | upstream-watchlist.md:279 | page names three locations, no order | unchanged | https://cursor.com/docs/cli/reference/configuration | "Global macOS/Linux ~/.cursor/cli-config.json"; "CURSOR_CONFIG_DIR : custom directory path"; "XDG_CONFIG_HOME (Linux/BSD): uses $XDG_CONFIG_HOME/cursor/cli-config.json" | same |
| 10 | `CURSOR_CONFIG_DIR` documented only on CLI config page; no page ties it to rules/skills/agents/mcp | upstream-watchlist.md:57; vendor_cursor.rs:24 | verified 2026-09-27 | unchanged (rules, skills, subagents, mcp pages grep clean for the variable) | https://cursor.com/docs/cli/reference/configuration | "Override with environment variables: CURSOR_CONFIG_DIR : custom directory path" | same |
| 11 | Project `.cursor/cli.json` `permissions` replaces global (on-limited); docs state no merge | upstream-watchlist.md:280; settings.md:965 | docs silent; forum 2026-05-16 | unchanged for docs: still no merge statement. Forum thread not re-fetched | https://cursor.com/docs/cli/reference/configuration | "Only permissions can be configured at the project level. All other CLI settings must be set globally." | same |
| 12 | `autoAcceptWebSearch` not written: changelog names it, reference omits it | upstream-watchlist.md:296 | not on reference | unchanged | https://cursor.com/docs/cli/changelog | "set autoAcceptWebSearch in cli-config.json . Honored in interactive, headless, subagent, and Agent Client Protocol runs." (reference page has no such row) | CLI v2026.07.13 |
| 13 | settings.md:1027 "CLI manages some fields and may overwrite them" | docs/src/content/docs/settings.md:1027 | restart assumed | unchanged | https://cursor.com/docs/cli/reference/configuration | "Some fields are CLI-managed and may be overwritten." | same |
| 14 | Config-string expansion: grim has no Cursor refusal; Cursor `cli-config.json` shows none. `mcp.json` `${env:NAME}` expands in `env` and in `auth` | vendor_cursor.rs:19, mcp-servers.md:222 | `${env:NAME}` | unchanged. `auth.CLIENT_ID`/`CLIENT_SECRET` also take `${env:...}`; no new expansion in cli-config | https://cursor.com/docs/context/mcp | "env No Environment variables for the server {"API_KEY": "${env:api-key}"}"; "CLIENT_ID": "${env:MCP_CLIENT_ID}" | docs 2026-11-16 |
| 15 | Trust gates: no cli-config key switches off workspace trust | settings rows | none proposed | unchanged: no trust-gate key on the configuration page | https://cursor.com/docs/cli/reference/configuration | (full key list read; none concerns trust) | same |
| 16 | MCP `auth` block: CLIENT_ID required, CLIENT_SECRET optional, scopes optional, two fixed redirects; no callback_port | upstream-watchlist.md:58 | skipped + warn | unchanged. Added: desktop-only redirect via static CLIENT_ID | https://cursor.com/docs/context/mcp | "CLIENT_ID Yes OAuth 2.0 Client ID from the MCP provider"; "Cursor uses fixed OAuth redirect URLs for MCP servers." | docs 2026-11-16 |
| 17 | Hooks: `allow`/`ask` verdicts not enforced | upstream-watchlist.md:254 | "staff-confirmed open bug" | unchanged, now documented | https://cursor.com/docs/hooks | ""ask" is accepted by the schema but not enforced for preToolUse today." | docs 2026-11-16 |
| 18 | Private repository import: GitLab/Bitbucket/Azure DevOps acceptance open | upstream-watchlist.md:219 | disagreement open (docs vs forum) | docs now state all four hosts accepted; auto-refresh needs the GitHub App | https://cursor.com/docs/plugins | "Import from Repo accepts GitHub, GitLab, Bitbucket, and Azure DevOps URLs. Connect that provider if Cursor asks." | docs 2026-11-16 |
| 19 | Plugin tree: root Agent Plugins `plugin.json` not in Cursor's lookup, hence second `.cursor-plugin/plugin.json` | upstream-watchlist.md:59 | root-only unproven | docs now document root `plugin.json` Agent Plugins layout as loading in Cursor; precedence vs `.cursor-plugin/` still unstated | https://cursor.com/docs/reference/plugins | "A plugin that conforms to the Agent Plugins specification loads in Cursor." | docs 2026-11-16 |
| 20 | Cursor does not expand `${PLUGIN_ROOT}`; grim renames Claude placeholders to it for Cursor | upstream-watchlist.md:59, :12 | not expanded | still not expanded. New: Cursor expands `${CURSOR_PLUGIN_ROOT}` AND `${CLAUDE_PLUGIN_ROOT}` (command, args, env values, cwd). grim's rename to `${PLUGIN_ROOT}` is the broken path; keeping `${CLAUDE_PLUGIN_ROOT}` or emitting `${CURSOR_PLUGIN_ROOT}` would resolve | https://cursor.com/docs/reference/plugins | "Cursor expands ${CURSOR_PLUGIN_ROOT} and ${CLAUDE_PLUGIN_ROOT} to the plugin's install path in command , args , env values, and cwd . It does not expand the standard's ${PLUGIN_ROOT} and ${PLUGIN_DATA} variables" | docs 2026-11-16 |
| 21 | Marketplace entry: schema has only name/source/description/minClientVersions; docs list `version` | upstream-watchlist.md:59 | `version` unproven | docs entry fields now: name, source, description, version, author, homepage, repository, license, keywords, logo, category, tags, skills/rules/agents/commands, hooks, mcpServers. `minClientVersions` not on page (schema not re-fetched) | https://cursor.com/docs/reference/plugins | "version string Semantic version"; "category string Plugin category"; "tags array Additional tags" | docs 2026-11-16 |
| 22 | Marketplace entry metadata `author` name+email only, no `author.url` | upstream-watchlist.md:60,63 | Cursor gets name+email | unchanged | https://cursor.com/docs/reference/plugins | "author object Author info: name (required), email (optional)" | docs 2026-11-16 |
| 23 | Marketplace entry `category` defined by Cursor | upstream-watchlist.md:61 | defined | unchanged | https://cursor.com/docs/reference/plugins | "category string Plugin category" | same |
| 24 | Marketplace-level logo/icon: none; Cursor `logo` per plugin | upstream-watchlist.md:45,46 | no marketplace logo | unchanged: top-level marketplace fields are name, owner, plugins, metadata (description, version, pluginRoot); `logo` only per plugin and per entry | https://cursor.com/docs/reference/plugins | "metadata object Optional. description , version , pluginRoot (prefix path for all plugin sources)"; "logo string Relative path to a logo file in the repo (e.g., assets/logo.svg ), or an absolute URL." | same |
| 25 | Skills: `.cursor/skills/`, `~/.cursor/skills/`, plus pool `.agents/skills`, `~/.agents/skills`; Claude/Codex dirs also read | vendor_cursor.rs:10; clients.md:426-455; vendor-metadata.md:464,488 | project/global as stated | unchanged | https://cursor.com/docs/context/skills | ".agents/skills/ Project-level"; ".cursor/skills/ Project-level"; "~/.agents/skills/ User-level (global) on the local machine"; "~/.cursor/skills/ User-level (global) on the local machine" | docs 2026-11-16 |
| 26 | Rules: `.cursor/rules/*.mdc`, `globs` comma-separated, `alwaysApply` | vendor_cursor.rs:12-14; clients.md:287; vendor-metadata.md:543 | as stated | unchanged. Forum #76648 not re-fetched | https://cursor.com/docs/context/rules | "Project rules must use the .mdc extension."; "Use globs to scope a rule to specific files or directories. Separate multiple patterns with commas." | docs 2026-11-16 |
| 27 | Agents: `.cursor/agents/`, `~/.cursor/agents/`; keys model, readonly, is_background; Claude/Codex dirs read | vendor_cursor.rs:15-17; vendor-metadata.md:324-336 | as stated | unchanged | https://cursor.com/docs/context/subagents | "model string No inherit Model to use: inherit or a specific model ID."; "readonly boolean No false"; "is_background boolean No false If true , the subagent runs in the background without blocking the parent." | docs 2026-11-16 |
| 28 | MCP paths `.cursor/mcp.json`, `mcpServers`, stdio `type`, `env`, `envFile` | vendor_cursor.rs:19; mcp-servers.md:222 | as stated | unchanged (path names in page: `mcp.json`; exact dirs not re-read in this pass) | https://cursor.com/docs/context/mcp | "configure custom servers with mcp.json"; "env No Environment variables for the server" | docs 2026-11-16 |

## Verifier sign-off

none required — no code landed in this run.

## Routing

- #8 Settings key list: reference lists keys grim has no row for → (b) additive renderer or metadata support — reported, not implemented in this run
- #10 CURSOR_CONFIG_DIR documented only on CLI config page; no page ties it to... → (a) ledger or doc text fix
- #11 Project .cursor/cli.json permissions replaces global (on-limited); docs state... → (a) ledger or doc text fix
- #14 Config-string expansion: grim has no Cursor refusal; Cursor cli-config.json... → (a) ledger or doc text fix
- #15 Trust gates: no cli-config key switches off workspace trust → (a) ledger or doc text fix
- #16 MCP auth block: CLIENT_ID required, CLIENT_SECRET optional, scopes optional,... → (a) ledger or doc text fix
- #17 Hooks: allow/ask verdicts not enforced → (a) ledger or doc text fix
- #18 Private repository import: GitLab/Bitbucket/Azure DevOps acceptance open → (a) ledger or doc text fix
- #19 Plugin tree: root Agent Plugins plugin.json not in Cursor's lookup, hence... → (c) issue draft
- #20 Cursor does not expand ${PLUGIN_ROOT}; grim renames Claude placeholders to it... → (c) issue draft
- #21 Marketplace entry: schema has only name/source/description/minClientVersions;... → (b) additive renderer or metadata support — reported, not implemented in this run
- #24 Marketplace-level logo/icon: none; Cursor logo per plugin → (a) ledger or doc text fix
- #26 Rules: .cursor/rules/*.mdc, globs comma-separated, alwaysApply → (a) ledger or doc text fix
- #28 MCP paths .cursor/mcp.json, mcpServers, stdio type, env, envFile → (a) ledger or doc text fix

## Issue drafts

### Cursor plugins: expand ${CURSOR_PLUGIN_ROOT} and ${CLAUDE_PLUGIN_ROOT}, and decide the dual-manifest layout

Cursor documents expansion of `${CURSOR_PLUGIN_ROOT}` and `${CLAUDE_PLUGIN_ROOT}` in command, args, env values and cwd, and now documents a root Agent Plugins `plugin.json` layout as loading in Cursor; precedence against `.cursor-plugin/plugin.json` is unstated. Source: https://cursor.com/docs/reference/plugins. grim renames Claude placeholders to `${PLUGIN_ROOT}` for Cursor, which Cursor does not expand, and emits a second `.cursor-plugin/plugin.json` because the root manifest was believed unread (`.agents/upstream-watchlist.md:59`, `:12`).

Labels: bug

## Friction

- dropped (unsourced): Forum #76648 (comma split inside brace glob), clients.md:287 and :785 — forum not re-fetched; docs page only confirms comma-separated globs.
- dropped (unsourced): Forum thread 160343 (project cli.json overrides global), watchlist:280 — not re-fetched.
- dropped (unsourced): Marketplace JSON schema (`marketplace.schema.json`, `plugin.schema.json`, `additionalProperties: false`) — not fetched; only docs page checked.
- dropped (unsourced): Exact `.cursor/mcp.json` / `~/.cursor/mcp.json` directory strings — MCP page greps show only "mcp.json"; not found verbatim in text pulled.
- dropped (unsourced): Hook-kind declined row (watchlist:234): grim-side schedule, no upstream fact to source.
- not reached: `research_upstream_cursor_20260927.md` and `research_cursor_plugin_probe.md` cross-checks.
- not reached: Team marketplace dashboard / `cursor-agent plugin marketplace add` interactive-only claim (watchlist:59 tail).
- not reached: `/migrate-to-skills` leaves `.mdc` shapes untouched (watchlist:57): command listed in skills docs ("/migrate-to-skills Converts eligible dynamic rules and slash commands into Agent Skills."), but its effect on `.mdc` not checked.
- not reached: vendor-metadata.md:130-152 subagent-support statements beyond the key table.
- feed: cursor heading was on https://cursor.com/changelog/page/2, not page 1.
- feed: newest "Voice Chat with Agents" (Nov 6, 2026), cursor found yes

## Skill changes

Host caps concurrent subagents at 20; the researcher built its own claim list; replies are a short summary with the table in a file. Fixed in this run's chore(skills) commit.
