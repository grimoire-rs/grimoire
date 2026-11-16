# Research: upstream refresh — cline (sweep)

## Metadata
**Date:** 2026-11-16 · **Domain:** vendors · **Depth:** sweep — checked 50d ago
**Triggered by:** /upstream-refresh
**Expires:** 2027-05-16

## Claims

| # | Claim | Ledger location | Old value | New value | Source URL | Verbatim quote | Vendor version |
|---|---|---|---|---|---|---|---|
| 1 | Settings: no key documented for `global-settings.json`; re-check trigger (docs list keys/types/values or a `cline config` verb prints them) | domains.md:109 | not fired | unchanged. Docs name the file only; `cline config` targets exclude settings | https://docs.cline.bot/cli/cli-reference.md | "To print one part of the configuration instead, for example in a script, name a target: `workflows`, `rules`, `skills`, `agents`, `plugins`, `hooks`, `mcp` or `tools`." | CLI v3.0.74 docs |
| 2 | MCP settings path: `CLINE_MCP_SETTINGS_PATH` > `$CLINE_DATA_DIR/settings/cline_mcp_settings.json` > `<CLINE_DIR\|~/.cline>/data/settings/...`; CLI and extension share it | upstream-watchlist.md:114; vendor_cline.rs:40; clients.md:182; mcp-servers.md:231 | source @252082b9 | unchanged in source; docs page no longer stale (now names the shared CLI path) | https://github.com/cline/cline/blob/v4.1.26/sdk/packages/shared/src/storage/paths.ts | `const explicitPath = process.env.CLINE_MCP_SETTINGS_PATH?.trim();` ... `return join(resolveClineDataDir(), "settings", CLINE_MCP_SETTINGS_FILE_NAME);` | v4.1.26 |
| 3 | Docs claim "CLI-only `~/.cline/mcp.json`" is stale | upstream-watchlist.md:114 | docs said CLI `~/.cline/mcp.json`, IDE UI-managed | docs now say `~/.cline/data/settings/cline_mcp_settings.json` for CLI, override `CLINE_MCP_SETTINGS_PATH`; ledger note can drop "stale" (a) | https://docs.cline.bot/mcp/mcp-overview.md | "**CLI:** `~/.cline/data/settings/cline_mcp_settings.json` (override with the `CLINE_MCP_SETTINGS_PATH` env var)" | CLI v3.0.74 docs |
| 4 | Extension shares same file (legacy-migration `getSharedMcpSettingsPath` honors the three vars) | upstream-watchlist.md:114 | @252082b9 | unchanged | https://github.com/cline/cline/blob/v4.1.26/apps/vscode/src/hosts/vscode/mcp-settings-legacy-migration.ts | `export function getSharedMcpSettingsPath(storage: StorageContext): string {` ... `const explicitPath = process.env.CLINE_MCP_SETTINGS_PATH?.trim()` | v4.1.26 |
| 5 | MCP lock: `SETTINGS_LOCK_STALE_MS = 10_000`, poll 25 ms, stale takeover renames to `.stale.<token>`, release unlinks marker then rmdir; CLI timeout 10 s | upstream-watchlist.md:115; vendor_cline.rs (cline_lock.rs) | @252082b9 | unchanged | https://github.com/cline/cline/blob/v4.1.26/apps/vscode/src/services/mcp/settingsLock.ts | `const SETTINGS_LOCK_STALE_MS = 10_000` / `const SETTINGS_LOCK_POLL_MS = 25` / `const staleDir = \`${lockDir}.stale.${makeLockToken()}\`` ; CLI `const timeoutMs = options.timeoutMs ?? SETTINGS_LOCK_STALE_MS;` (sdk/.../mcp/config-loader.ts) | v4.1.26 |
| 6 | Only the extension expands `${env:VAR}`; CLI loader expands nothing | upstream-watchlist.md:116; mcp-servers.md:316; clients.md:194 | @252082b9 | unchanged (envExpansion.ts regex still `${env:...}`; McpHub still calls `expandEnvironmentVariables`; no expansion in CLI config-loader) | https://github.com/cline/cline/blob/v4.1.26/apps/vscode/src/utils/envExpansion.ts | `return value.replace(/\$\{env:([^}]+)\}/g, (match, varName) => {` | v4.1.26 |
| 7 | Oauth: Cline keeps own `oauthClient`/`oauth` state; no mapping | upstream-watchlist.md:116; vendor_cline.rs:58 | unverified shape | unchanged: schemas still carry `oauthClient: oauthClientSchema.optional(), oauth: oauthStateSchema.optional()` | https://github.com/cline/cline/blob/v4.1.26/apps/vscode/src/services/mcp/schemas.ts | `oauthClient: oauthClientSchema.optional(),` / `oauth: oauthStateSchema.optional(),` | v4.1.26 |
| 8 | Flat entry form, `type` stdio/sse/streamableHttp; omitted type defaults to legacy sse | upstream-watchlist.md:114; mcp-servers.md:231 | stdio\|sse\|streamableHttp | unchanged; docs add that omitting `type` means sse (grim always writes `type`) | https://docs.cline.bot/mcp/mcp-overview.md | "The `type` field selects the transport. Omitting it defaults to the legacy `sse` transport for backward compatibility, so set `\"type\": \"streamableHttp\"` explicitly for the recommended transport." | CLI v3.0.74 docs |
| 9 | Agents: CLI reads `.cline/agents/` and `~/.cline/agents/`; format "Markdown with YAML frontmatter" (`name`,`description`,`tools`,`skills`) | upstream-watchlist.md:112; vendor_cline.rs:32-39; clients.md:568 | Markdown + YAML frontmatter | paths unchanged; **file extension is `.yml`/`.yaml` only**, not `.md` (same at v4.1.21): ledger format text wrong, fix text (a). Decline stands | https://github.com/cline/cline/blob/v4.1.26/sdk/packages/core/src/extensions/tools/team/configured-agent-config.ts | `return extension === ".yml" \|\| extension === ".yaml";` and `if (!entry.isFile() \|\| !isYamlFile(entry.name)) {` | v4.1.26 (also v4.1.21) |
| 10 | Extension hardcodes `enableSpawnAgent: false` (re-check trigger: extension starts loading agents) | upstream-watchlist.md:112; vendor_cline.rs:38 | cline-session-factory.ts:1087 @v4.1.21 | still false; line moved to 1191 @v4.1.26; docs agree. Trigger not fired | https://docs.cline.bot/features/subagents.md | "Subagents are available in the Cline CLI and Cline Desktop. The VS Code and JetBrains extensions don't use them." | v4.1.26 |
| 11 | Skills pool: CLI `resolveSkillsConfigSearchPaths` and extension `getSkillsDirectoriesForScan` both scan project `.agents/skills` and global `~/.agents/skills` | upstream-watchlist.md:117; vendor_cline.rs:16; clients.md:427,585 | @v4.1.21 | unchanged; both still list them. Docs now confirm `.agents/skills` | https://docs.cline.bot/getting-started/config.md | "skills in `.agents/skills/`. Globally, it also reads `~/.agents/AGENTS.md` and skills in `~/.agents/skills/`." | v4.1.26 |
| 12 | Skills precedence "`.cline/` -> `.clinerules/` -> `.claude/`"; `.cline/skills` "first in Cline's own precedence" | vendor-metadata.md:468; vendor_cline.rs:11 | `.cline/` first | **source scan order is `.clinerules`, `.cline`, `.claude`, `.agents`** (both CLI and extension). Docs list `.cline/skills/` as "(recommended)" with no precedence statement. Fix text (a) | https://github.com/cline/cline/blob/v4.1.26/apps/vscode/src/core/storage/skill-directories.ts | `{ path: path.join(cwd, SKILL_DIRECTORY_NAMES.clineruleSkillsDir), source: "project" },` listed before `clineSkillsDir` | v4.1.26 |
| 13 | New: same-name skill, global vs project | (no row) | none | docs: global wins over project. Bears on a grim project install shadowed by a global skill of the same name | https://docs.cline.bot/customization/skills.md | "When a global skill and project skill have the same name, the global skill takes precedence." | v4.1.26 docs |
| 14 | `CLINE_DATA_DIR` only feeds `resolveClineDataDir()`, never skills; `CLINE_DIR` relocates CLI skills root but extension hardcodes `os.homedir()/.cline` | upstream-watchlist.md:111; vendor-metadata.md:496 | @v4.1.21 | unchanged. Extension: `function getClineHomePath(): string { return path.join(os.homedir(), ".cline") }`. CLI: `join(resolveClineDir(), SKILLS_CONFIG_DIRECTORY_NAME)`. Revisit trigger not fired. Docs now document both vars | https://github.com/cline/cline/blob/v4.1.26/apps/vscode/src/core/storage/skill-directories.ts | `return path.join(os.homedir(), ".cline")` | v4.1.26 |
| 15 | Docs env var table | upstream-watchlist.md:111 | `CLINE_DIR`, `CLINE_DATA_DIR`, `CLINE_MCP_SETTINGS_PATH` | unchanged; docs table lists `CLINE_DIR`, `CLINE_DATA_DIR`; also new `CLINE_HOOKS_DIR` (hooks, declined, no row) | https://docs.cline.bot/getting-started/config.md | "`CLINE_DIR` \| Custom configuration directory (replaces `~/.cline/`), like `--config`" ; "`CLINE_DATA_DIR` \| Custom data directory (replaces `~/.cline/data/`)" | v4.1.26 docs |
| 16 | Rules: `paths` is the supported conditional (rule decline is scheduling, capability exists) | upstream-watchlist.md:110; clients.md:564,581 | features/cline-rules.md | unchanged text; page now at customization/cline-rules.md (old URL redirects); `.cline/rules/` also supported beside `.clinerules/` | https://docs.cline.bot/customization/cline-rules.md | "Currently, `paths` is the supported conditional. It takes an array of glob patterns:" | v4.1.26 docs |
| 17 | Global rules also read from `~/Cline/Rules` and `~/.cline/rules` | upstream-watchlist.md:110 | since CLI 3.0.56 | unchanged | https://docs.cline.bot/customization/cline-rules.md | "Cline also searches `~/.cline/rules` and `~/Cline/Rules` for global rules." | v4.1.26 docs |
| 18 | Hooks declined: Cline owns a hooks dir of its own | upstream-watchlist.md:235 | whole-file ownership shape | unchanged; source `resolveHooksConfigSearchPaths` = `~/Documents/Cline/Hooks`, `~/.cline/hooks`, `.clinerules/hooks`, `.cline/hooks`; docs add `CLINE_HOOKS_DIR`. CLI v3.0.70 added `cline hook` `agent_error` events | https://github.com/cline/cline/blob/v4.1.26/sdk/packages/shared/src/storage/paths.ts | `join(workspacePath, DEPRECATED_CONFIG_DIR, HOOKS_CONFIG_DIRECTORY_NAME),` / `join(workspacePath, CLINE_CONFIG_DIR, HOOKS_CONFIG_DIRECTORY_NAME),` | v4.1.26 |
| 19 | docs link `https://docs.cline.bot/mcp/configuring-mcp-servers` | mcp-servers.md:575 | live page | redirects to `https://docs.cline.bot/mcp/mcp-overview` (200); update link (a) | https://docs.cline.bot/mcp/mcp-overview.md | curl -L final URL https://docs.cline.bot/mcp/mcp-overview | n/a |
| 20 | Wave-2 batch date text "All rows verified 2026-07-27 ... unless the cell carries a newer date" | upstream-watchlist.md:102 | date | stamps stay 2026-09-27 on all Cline rows; bump to 2026-11-16 where rows 1-18 confirm (a) | n/a (ledger) | n/a | n/a |

## Verifier sign-off

none required — no code landed in this run.

## Routing

- #1 Settings: no key documented for global-settings.json; re-check trigger (docs... → (a) ledger or doc text fix
- #2 MCP settings path: CLINE_MCP_SETTINGS_PATH >... → (a) ledger or doc text fix
- #3 Docs claim "CLI-only ~/.cline/mcp.json" is stale → (a) ledger or doc text fix
- #6 Only the extension expands ${env:VAR}; CLI loader expands nothing → (a) ledger or doc text fix
- #7 Oauth: Cline keeps own oauthClient/oauth state; no mapping → (a) ledger or doc text fix
- #8 Flat entry form, type stdio/sse/streamableHttp; omitted type defaults to legacy... → (a) ledger or doc text fix
- #9 Agents: CLI reads .cline/agents/ and ~/.cline/agents/; format "Markdown with... → (a) ledger or doc text fix
- #10 Extension hardcodes enableSpawnAgent: false (re-check trigger: extension starts... → (a) ledger or doc text fix
- #11 Skills pool: CLI resolveSkillsConfigSearchPaths and extension... → (a) ledger or doc text fix
- #12 Skills precedence ".cline/ -> .clinerules/ -> .claude/"; .cline/skills "first... → (a) ledger or doc text fix
- #13 New: same-name skill, global vs project → (a) ledger or doc text fix
- #14 CLINE_DATA_DIR only feeds resolveClineDataDir(), never skills; CLINE_DIR... → (a) ledger or doc text fix
- #15 Docs env var table → (a) ledger or doc text fix
- #16 Rules: paths is the supported conditional (rule decline is scheduling,... → (a) ledger or doc text fix
- #18 Hooks declined: Cline owns a hooks dir of its own → (a) ledger or doc text fix
- #19 docs link https://docs.cline.bot/mcp/configuring-mcp-servers → (a) ledger or doc text fix
- #20 Wave-2 batch date text "All rows verified 2026-07-27 ... unless the cell... → (a) ledger or doc text fix

## Issue drafts

none

## Friction

- dropped (unsourced): vendor_cline.rs:6 "re-verified" docs-page claims (skills page raw markdown, three-entry precedence): the current skills page states no precedence order, only a list; covered by row 12.
- dropped (unsourced): Whether `.claude/skills` scanning (v4.1.24 / CLI 3.0.71 "skills in `.claude/skills` are now discovered") is new in the CLI or already worked in the extension at v4.1.21: not compared (v4.1.21 CLI paths.ts not fetched). No grim impact, grim writes `.cline/skills`.
- dropped (unsourced): Duplicate-skill-name resolution inside the CLI (which dir wins): not read.
- not reached: docs/src/content/docs/settings.md (no Cline statement found by grep; nothing to verify).
- not reached: Windows `%USERPROFILE%\.cline\skills\` (vendor-metadata.md:496): docs list `C:\Users\USERNAME\.cline\skills\`, consistent; not source-checked.
- feed: newest v4.1.26 (2026-10-26), cursor found yes

## Skill changes

Host caps concurrent subagents at 20; the researcher built its own claim list; replies are a short summary with the table in a file. Fixed in this run's chore(skills) commit.
