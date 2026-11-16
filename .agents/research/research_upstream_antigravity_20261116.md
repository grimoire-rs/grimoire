# Research: upstream refresh — antigravity (sweep)

## Metadata
**Date:** 2026-11-16 · **Domain:** vendors · **Depth:** sweep — checked 50d ago
**Triggered by:** /upstream-refresh
**Expires:** 2027-05-16

## Claims

| # | Claim | Ledger location | Old value | New value | Source URL | Verbatim quote | Vendor version |
|---|---|---|---|---|---|---|---|
| 1 | Settings key list: 25 grim rows still exist upstream with same type/enum | src/install/vendor_antigravity.rs:89-158; .agents/upstream-watchlist.md:301 | 25 rows, `/agentMode` omits `""` | unchanged for all 25 keys (type, enum literals, defaults match schema). Schema also lists 4 more scalar keys grim leaves off (`/editor`, `/historySize`, `/modelProvider`, `/useG1Credits`) and objects (`trustedWorkspaces`, `hooks`, `statusLine`, `title`, `sandboxProxy`, `customModelsConfig`, `gcp`, `enableTelemetry`), all already ruled off in research_upstream_antigravity_settings_20261105.md rows 27-32 | https://antigravity.google/schemas/v1/settings.json | `"agentMode": {"enum": ["", "accept-edits", "auto", "plan"]}` (schema, enum list as fetched); `"additionalProperties": true` at root | CLI 1.5.8 (docs) |
| 2 | `/agentMode` `""` is the CLI default so grim omits it | vendor_antigravity.rs:86-88; watchlist:301 | `""` is default, CLI deletes default-valued key | CONFLICT: CLI 1.5.7 changed default to Auto; `""` is now an explicit "Default" choice that is saved. Settings page and CLI reference still say `""`/default is the default (docs lag the changelog) | https://antigravity.google/docs/changelog | "Changed the default Agent Mode from Default to Auto: sessions with no saved mode, including headless -p runs, now start in Auto wherever Auto mode is available and in Default elsewhere; to keep the previous behavior, choose Default for Agent Mode in /config, which saves "agentMode": "" to settings.json." | CLI 1.5.7 (2026-11-12) |
| 3 | Default-valued keys dropped at startup | watchlist:308 | quote "When the CLI starts with a valid file, it rewrites the file in its own format, which also removes keys that are set to their default values." | unchanged (text still present; but see #2 for `agentMode`) | https://antigravity.google/docs/settings.md | "When the CLI starts with a valid file, it rewrites the file in its own format, which also removes keys that are set to their default values." | CLI 1.5.8 |
| 4 | `/permissions/allow`,`/ask`,`/deny` global only, project precedence | watchlist:309; vendor_antigravity.rs:110-112 | settings page "combines them with the rules in the Project and Shared scopes"; changelog v1.0.12 project "take precedence" | unchanged | https://antigravity.google/docs/settings.md ; https://antigravity.google/docs/changelog | "These are the rules in the **Global** scope of [`/permissions`](/docs/cli/commands/permissions). The CLI combines them with the rules in the **Project** and **Shared** scopes." / "Improved permission config merging priorities by ensuring project-specific configurations (located in ~/.gemini/config/projects/) take precedence over global settings in ~/.gemini/antigravity-cli/settings.json." | CLI 1.0.12 / settings page at 1.5.8 |
| 5 | Settings path `~/.gemini/antigravity-cli/settings.json`, global only | domains.md:101; watchlist:301 | `settings.json` in `antigravity-cli-root` | unchanged. New: hooks do not live in settings.json (`hooks.json` files) | https://antigravity.google/docs/settings.md | "Hooks don’t go in `settings.json`: the CLI keeps a `hooks` key there but never loads it." and path line `~/.gemini/antigravity-cli/settings.json` | CLI 1.5.8 |
| 6 | Expansion of variables / file refs in settings strings | Template claim | none known | not documented for string values; only `statusLine`/`title` `command` expands a leading `~`, `editor` auto uses `$EDITOR` (CLI-side). No `{env:`/`$NAME` substitution documented | https://antigravity.google/docs/settings.md | "A leading `~` expands to your home directory." (statusLine/title command) | CLI 1.5.8 |
| 7 | Trust gate: no key that pre-trusts a folder is proposed | domains.md:88 | `/trustedWorkspaces` excluded | unchanged, still CLI-managed and still excluded | https://antigravity.google/schemas/v1/settings.json | `trustedWorkspaces` array default `[]`; settings page: "Folders you chose to trust when the CLI asked. The CLI adds entries for you." | CLI 1.5.8 |
| 8 | Rules: path `.agents/rules/<n>.md`, `~/.gemini/config/rules/<n>.md`; frontmatter required; trigger literals; globs comma-separated; description required for model_decision | watchlist:88; vendor_antigravity.rs:34-45 | as ledger | unchanged (raw `.md` fetch, not summarizer) | https://antigravity.google/docs/rules.md | "**Every `.md` file inside `rules/` must start with YAML frontmatter** declaring a valid `trigger`." / `description: "..." # Required for model_decision; recommended for all rules` / `globs: "*.ts, *.tsx" # Required for glob (singular `glob:` is also accepted)` | 2.32.0 |
| 9 | Rules: only immediate `.md` children load; nested only via `.agents/rules.json` | watchlist:88 | as ledger | unchanged | https://antigravity.google/docs/rules.md | "Antigravity scans only immediate `.md` children inside `.agents/rules/`" ... "ignores files nested in subdirectories ... unless you explicitly register them in `.agents/rules.json`." | 2.32.0 |
| 10 | Rules limits: 24 KB per file, 20,000-token budget for global + always_on | watchlist:88 | as ledger | unchanged | https://antigravity.google/docs/rules.md | "**24 KB (`24,000` bytes) per-file limit**" and "**20,000-token aggregate rules budget**: All active global and `always_on` rules share a 20,000-token budget" | 2.32.0 |
| 11 | Missing frontmatter silently discards the rule | watchlist:88 | "silently discard[ed]" | unchanged | https://antigravity.google/docs/rules.md | "Antigravity silently discards the rule." | 2.32.0 |
| 12 | Frontmatter YAML parse bug for unquoted colon descriptions | watchlist:88 (description derivation) | not covered | new fix: single-line unquoted descriptions with colons used to error; fixed. Relevant to grim emitting `description` | https://antigravity.google/docs/changelog | "Fixed an issue where single-line unquoted description fields containing colons caused YAML frontmatter parsing errors in skills and rules." | 2.25.0 (2026-10-20) |
| 13 | Skills paths: global `~/.gemini/config/skills/<f>/`, workspace `.agents/skills/<f>/`; CLI also reads `~/.gemini/antigravity-cli/skills/` and `~/.gemini/skills/`; legacy `~/.gemini/antigravity/skills/` | vendor_antigravity.rs:7-31; watchlist:90,97 | as ledger | unchanged | https://antigravity.google/docs/skills.md | "`~/.gemini/config/skills/<skill-folder>/` \| Global (all workspaces, shared with the Antigravity App)" and "The Antigravity CLI also loads skills from `~/.gemini/antigravity-cli/skills/` and `~/.gemini/skills/`, which the Antigravity App doesn’t read." | 2.32.0 / CLI 1.5.8 |
| 14 | Agents: `.agents/agents/<n>.md`, `~/.gemini/config/agents/<n>.md`; frontmatter name/description/tools string[]/model | vendor_antigravity.rs:32-34; watchlist:94 | as ledger | paths and keys unchanged. `model` values now documented as `inherit`, `flash` or `pro` (grim projects `model` verbatim: check value mapping). Registry candidate keys unchanged: `mainAgent`, `subagent`, `commandExecutionPolicy`, `mcpServers`, `skills`/`plugins` | https://antigravity.google/docs/subagents.md | "`model` \| `string` \| `inherit` \| Model tier used when invoked (`inherit`, `flash`, or `pro`)." and "**Workspace agents**: `.agents/agents/<name>.md` or `.agents/agents/<name>/agent.md`" | 2.32.0 |
| 15 | MCP paths: global `~/.gemini/config/mcp_config.json`, workspace `.agents/mcp_config.json`; `mcpServers` map; `serverUrl` for remote | vendor_antigravity.rs:46-50 | as ledger | unchanged | https://antigravity.google/docs/mcp.md | "**Global configuration**: `~/.gemini/config/mcp_config.json`, which applies to all your projects." / "**Workspace configuration**: `<workspace>/.agents/mcp_config.json`" | 2.32.0 / CLI 1.5.8 |
| 16 | `ws` MCP transport declined (old sentence named websocket) | watchlist:92 | doc said "SSE, Streamable HTTP, or websocket-based MCP connections, you must define the `serverUrl` field" | CHANGED: that sentence is gone. Remote must be Streamable HTTP only; legacy SSE and `httpUrl` unsupported; websocket not mentioned. `ws` stays declined | https://antigravity.google/docs/mcp.md | "Remote MCP servers must use the Streamable HTTP transport. Set the server’s endpoint in `serverUrl` (`url` also works). The legacy HTTP+SSE transport and the `httpUrl` field aren’t supported." | 2.32.0 |
| 17 | MCP oauth block skipped; manual mode takes `clientId`, `clientSecret` | watchlist:95 | `clientId`, `clientSecret` | clientSecret now documented optional. Rest unchanged | https://antigravity.google/docs/mcp.md | "Manual OAuth 2.0 client credentials (`clientId` and optional `clientSecret`) for remote servers that do not support Client ID Metadata Documents (CIMD) or Dynamic Client Registration (DCR)." | 2.32.0 |
| 18 | MCP env refs: no `${VAR}` substitution documented | watchlist:96 | silence | unchanged: `env` is a plain key-value map injected into the stdio process; no substitution text found (grep `${` over page: no hits) | https://antigravity.google/docs/mcp.md | "Key-value map of environment variables injected into the local `stdio` server process." | 2.32.0 |
| 19 | Project-scope detection: repository settings load from `/.gemini/config.json`, `.agents/settings.json` no longer read | watchlist:89 | changelog sentence | unchanged (entry still in 2.17.0 notes) | https://antigravity.google/docs/changelog | "Repository customization settings now load from /.gemini/config.json. The older .agents/settings.json is no longer read" (text split across tags; reads as in ledger) | 2.17.0 |
| 20 | CLI/IDE variants served by one client; settings at `~/.gemini/antigravity-cli/settings.json`; rules/mcp/subagents say Available on CLI | watchlist:97 | as ledger | unchanged | https://antigravity.google/docs/rules.md | "Available on: Antigravity App, Antigravity CLI, Antigravity IDE" and CLI global rules "`~/.gemini/config/rules/*.md`" | 2.32.0 |
| 21 | Path-relocating env override: none honored | watchlist:93 | none | unchanged for skills/mcp/rules/subagents pages. New env vars exist but none relocate config: `ANTIGRAVITY_DISABLE_EMBEDDED_RIPGREP`, `AGY_ADC_AUTH`, `AGY_LLM_GATEWAY_MODELS`, `AGY_CLI_MAC_OPTION_AS_ALT`, `GEMINI_API_KEY` | https://antigravity.google/docs/changelog | "Added the ANTIGRAVITY_DISABLE_EMBEDDED_RIPGREP environment variable, which you can set to true to stop the CLI from extracting and running its bundled ripgrep" | CLI 1.5.5 |
| 22 | Hook kind declined; shell-command hooks spliced into a config file | watchlist:234 | "splices shell-command hooks into a shared config file" | NEW FACT for the flip: hooks live in dedicated files, not settings.json: `~/.gemini/config/hooks.json` global, `<workspace>/.agents/hooks.json` per workspace | https://antigravity.google/docs/settings.md | "**Hooks**: `~/.gemini/config/hooks.json` for all workspaces, or `<workspace>/.agents/hooks.json` for one workspace." | CLI 1.5.8 |
| 23 | `antigravity.*` agent registry candidates | watchlist:94 | `hooks:` (2.17.0), `rules:` (2.11.0), `inheritCustomizations` (2.9.1) joined list | the current subagents frontmatter table lists `name, description, tools, mainAgent, subagent, model, commandExecutionPolicy, mcpServers, skills/plugins`; `hooks:`/`rules:`/`inheritCustomizations` absent from that table now (changelog 2.23.0 still says custom agents use "the skills, rules, plugins, and hooks it listed") | https://antigravity.google/docs/subagents.md | "`skills` / `plugins` \| `string[]` \| `[]` \| Skill paths (for example, `skills/my-helper-skill`) or plugin dependencies." | 2.32.0 |

## Verifier sign-off

Signed off 2026-11-16 by three read-only sonnet verifiers (upstream re-fetched, grim code read) and by the implementing workers; one line per code-driving claim.

- #2 `/agentMode` `""`: confirmed in the CLI 1.5.7 changelog; landed in `58754de8` (grim accepts the empty string; the settings page still calls `""` the default).
- MCP SSE / `httpUrl` finding: refuted, NOT REAL; grim writes `serverUrl` only (driver verification 2026-11-16).

## Routing

- #1 Settings key list: 25 grim rows still exist upstream with same type/enum → (a) ledger or doc text fix
- #2 /agentMode "" is the CLI default so grim omits it → (b) landed in `58754de8` (was an issue draft)
- #3 Default-valued keys dropped at startup → (a) ledger or doc text fix
- #5 Settings path ~/.gemini/antigravity-cli/settings.json, global only → (a) ledger or doc text fix
- #6 Expansion of variables / file refs in settings strings → (a) ledger or doc text fix
- #7 Trust gate: no key that pre-trusts a folder is proposed → (a) ledger or doc text fix
- #8 Rules: path .agents/rules/<n>.md, ~/.gemini/config/rules/<n>.md; frontmatter... → (a) ledger or doc text fix
- #12 Frontmatter YAML parse bug for unquoted colon descriptions → (a) ledger or doc text fix
- #14 Agents: .agents/agents/<n>.md, ~/.gemini/config/agents/<n>.md; frontmatter... → (a) ledger or doc text fix
- #16 ws MCP transport declined (old sentence named websocket) → (a) ledger or doc text fix
- #17 MCP oauth block skipped; manual mode takes clientId, clientSecret → (a) ledger or doc text fix
- #18 MCP env refs: no ${VAR} substitution documented → (a) ledger or doc text fix
- #19 Project-scope detection: repository settings load from /.gemini/config.json,... → (a) ledger or doc text fix
- #21 Path-relocating env override: none honored → (a) ledger or doc text fix
- #22 Hook kind declined; shell-command hooks spliced into a config file → (c) issue draft
- #23 antigravity.* agent registry candidates → (a) ledger or doc text fix
- MCP SSE / `httpUrl` → dropped: not real, grim writes `serverUrl` only

## Issue drafts

### Antigravity hooks: dedicated hooks.json files (findings only)

Antigravity hooks live in dedicated files, not settings.json: `~/.gemini/config/hooks.json` globally and `<workspace>/.agents/hooks.json` per workspace. Source: https://antigravity.google/docs/settings.md. grim declines the hook kind on the premise of a splice into a shared config file (`.agents/upstream-watchlist.md:234`). Hooks are findings only; no renderer work is proposed.

Labels: enhancement

## Friction

- dropped (unsourced): Raw-fetch quote for row 19's exact changelog sentence: the changelog is an HTML page; text is split across tags, quote reassembled from stripped text.
- dropped (unsourced): Row 1 enum quote is from the JSON schema as parsed with python; original JSON formatting may differ.
- not reached: Docs statements in docs/src/content/docs/clients.md, vendor-metadata.md, mcp-servers.md, settings.md that rest on upstream facts (no row-by-row recheck; rows above cover the same upstream facts).
- not reached: `//! verified` stamps at vendor_antigravity.rs:458-459 and "checked live 2026-11-05 on agy 1.5.0" (line 9): live check not repeatable without the binary; page-level equivalents covered by rows 13, 15, 20.
- not reached: Rows for Goose agent-path (watchlist:121): not antigravity's.
- not reached: mcp `serverUrl` vs `url`, `disabled`, `tools.eager` per-key behavior beyond rows above.
- feed: /changelog/ redirects to /docs/changelog; fetched with curl and HTML stripped.
- feed: newest v2.32.0 (2026-11-13), cursor found yes

## Skill changes

Host caps concurrent subagents at 20; the researcher built its own claim list; replies are a short summary with the table in a file. Fixed in this run's chore(skills) commit.
