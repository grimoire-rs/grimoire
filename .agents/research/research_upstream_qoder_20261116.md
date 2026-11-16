# Research: upstream refresh — qoder (sweep)

## Metadata
**Date:** 2026-11-16 · **Domain:** vendors · **Depth:** sweep — checked 50d ago
**Triggered by:** /upstream-refresh
**Expires:** 2027-05-16

## Claims

| # | Claim | Ledger location | Old value | New value | Source URL | Verbatim quote | Vendor version |
|---|---|---|---|---|---|---|---|
| 1 | Settings key `/model/name` string | src/install/vendor_qoder.rs:62 | string, scalar, global | unchanged | https://docs.qoder.com/cli/settings-reference.md | `model.name` / "string" / "None" | CLI 1.1.90 (docs current 2026-11-16) |
| 2 | `/general/defaultPermissionMode` enum 6 values | vendor_qoder.rs:63-74 | default, accept_edits, plan, auto, bypass_permissions, dont_ask | unchanged | https://docs.qoder.com/cli/settings-reference.md | "Default Permission Mode: `default`/`accept_edits`/`plan`/`auto`/`bypass_permissions`/`dont_ask`." | 1.1.90 |
| 3 | `/general/vimMode`, `/enableAutoUpdate`, `/enableNotifications` bool | vendor_qoder.rs:76-78 | bool, scalar | unchanged | https://docs.qoder.com/cli/settings-reference.md | `general.vimMode` / "boolean" / "false"; `general.enableAutoUpdate` / "boolean" / "true"; `general.enableNotifications` / "boolean" / "false" | 1.1.90 |
| 4 | `/output/format` enum text,json | vendor_qoder.rs:79 | text, json | unchanged | https://docs.qoder.com/cli/settings-reference.md | "CLI output format: `text` or `json`." | 1.1.90 |
| 5 | `/ui/theme` string; `/ui/hideTips`, `/hideBanner`, `/showLineNumbers`, `/compactToolOutput` bool | vendor_qoder.rs:80-84 | string / bool | unchanged | https://docs.qoder.com/cli/settings-reference.md | `ui.theme` / "string"; `ui.hideTips` / "boolean"; `ui.hideBanner` / "boolean"; `ui.showLineNumbers` / "boolean"; `ui.compactToolOutput` / "boolean" | 1.1.90 |
| 6 | `/permissions/allow`, `/ask`, `/deny` string list, Set merge, global | vendor_qoder.rs:85-87 | string[] | unchanged | https://docs.qoder.com/cli/settings-reference.md | `permissions.allow` / "string[]" / "[]"; `permissions.ask` / "string[]"; `permissions.deny` / "string[]" | 1.1.90 |
| 7 | `/security/disableYoloMode` bool | vendor_qoder.rs:88 | bool | unchanged | https://docs.qoder.com/cli/settings-reference.md | "Disable `bypass_permissions` (YOLO) Permission Mode." | 1.1.90 |
| 8 | `/model/reasoningEffort` enum, eight values | vendor_qoder.rs:89-93; upstream-watchlist.md:300 | disabled, off, none, low, medium, high, xhigh, max (verified 2026-11-05, 1.1.83) | unchanged | https://docs.qoder.com/cli/settings-reference.md | "Reasoning Effort: `disabled`/`off`/`none`/`low`/`medium`/`high`/`xhigh`/`max`." | 1.1.90 |
| 9 | Settings watchlist `/permissions/*` `on-limited`: pages disagree on union vs replace | upstream-watchlist.md:281 | pages disagree; limit kept | unchanged | https://docs.qoder.com/cli/permissions.md ; https://docs.qoder.com/cli/settings.md | permissions: "Higher-priority sources override lower ones." settings: arrays "use "union merge" to combine and deduplicate values across levels" for disable/exclude lists; other arrays overridden by the higher-precedence value. Neither page states a union for `permissions.*` | 1.1.90 |
| 10 | Expansion of variables in config strings (grim refuses none for qoder settings) | upstream-watchlist.md:147 (nearest row; no settings-expansion row) | no expansion documented in docs; bundle-only (1.1.64) | NEW: settings page now says env vars can be referenced in values | https://docs.qoder.com/cli/settings.md | "Environment variables can be referenced in values and will be resolved and replaced at runtime." (syntax not stated; settings-reference has no such sentence; mcp-reference has no `${VAR}`) | 1.1.90 |
| 11 | Trust gates: no trust-gate key proposed as a row | vendor_qoder.rs:61-94; domains.md:83-91 | none in rows; qoder not in the known-keys list | NEW findings: `security.folderTrust.enabled` and `permissions.trustDirectories` exist upstream (not proposed as rows) | https://docs.qoder.com/cli/settings-reference.md | "Enable Folder Trust." ; "Directories explicitly trusted by the user; stored in user-level configuration" | 1.1.90 |
| 12 | Support-dir exclusion: `agentsMdExcludes` still does not cover `rules/` | upstream-watchlist.md:146 | `string[]` default `[]`; scope of `rules/**` unstated | unchanged (text now adds "Top-level key, supports manual configuration only") | https://docs.qoder.com/cli/settings-reference.md ; https://docs.qoder.com/cli/memory.md | "Exclude project-level / local-level memory files by glob. Top-level key, supports manual configuration only." ; "Check if `agentsMdExcludes` excludes the target file." | 1.1.90 |
| 13 | Rules load recursively `rules/**/*.md`; `paths` native | upstream-watchlist.md:146; vendor_qoder.rs:9-14; vendor-metadata.md:544 | recursive; `paths` verbatim | unchanged | https://docs.qoder.com/cli/memory.md | project-level at `<project>/.qoder/rules/**/*.md` and User-Level `~/.qoder/rules/**/*.md`; "`glob` (used with `trigger: glob`) and `paths` both set the file scope." (summarized fetch) | 1.1.90 |
| 14 | `QODER_CONFIG_DIR` honored, replaces `~/.qoder` | upstream-watchlist.md:173; clients.md:714; vendor-metadata.md:502 | quote from config-scope | unchanged | https://docs.qoder.com/cli/config-scope.md | "The location of the User Configuration Directory can be customized via the `QODER_CONFIG_DIR` Environment Variable." | 1.1.90 |
| 15 | MCP env-ref expansion not documented in mcp-reference | upstream-watchlist.md:147 | `env` = "Environment variables passed to the subprocess"; no substitution | unchanged for mcp-reference (see row 10 for settings page) | https://docs.qoder.com/cli/mcp-reference.md | "Environment variables passed to the subprocess." (page has no `${VAR}` syntax) | 1.1.90 |
| 16 | MCP `ws` transport uses `tcp` host/port | upstream-watchlist.md:148 | tcp {host,port} | unchanged | https://docs.qoder.com/cli/mcp-reference.md | "TCP connection parameters (host/port)." | 1.1.90 |
| 17 | MCP `oauth` field list | upstream-watchlist.md:149 | enabled, clientId, clientSecret, authorizationUrl, tokenUrl, scopes, callbackPort | unchanged | https://docs.qoder.com/cli/mcp-reference.md | "fields include `enabled`, `clientId`, `clientSecret`, `authorizationUrl`, `tokenUrl`, `scopes`, `callbackPort`, etc." | 1.1.90 |
| 18 | Project MCP file: settings.json needs approval; override order | upstream-watchlist.md:150 | quotes recorded 2026-09-27 | unchanged | https://docs.qoder.com/cli/mcp-reference.md | "Requires approval before use (for security reasons)." ; "User Level → Project-Level `settings.json` → Project-Level `.mcp.json` → Local Level → CLI argument." | 1.1.90 |
| 19 | `.agents/skills` pool: not pool-capable; changelog-only evidence | upstream-watchlist.md:152 | 1.1.7 toggle, 1.1.11 default on, 1.1.9 recursive | unchanged; no 1.1.65-1.1.90 entry mentions `.agents` | https://changelog.qoder.com/cli (docs.qoder.com/release-notes/qoder-cli 307-redirects here) | 1.1.11: "The .agents/skills compatibility source is now enabled by default" ; 1.1.9: "Skill directories are now scanned recursively, with a visible warning when a declared directory contains no skills" | 1.1.7/1.1.9/1.1.11; current 1.1.90 |
| 20 | Plugin manifest `.qoder-plugin/plugin.json`; marketplace required keys name, owner, plugins; no rules component | upstream-watchlist.md:154 | as stated | unchanged (docs part; `./` rule and `.claude-plugin` fallback are bundle-only, not in docs) | https://docs.qoder.com/cli/plugins-reference.md | "The manifest file is located at `.qoder-plugin/plugin.json`" ; supported components: commands, agents, skills, outputStyles, workflows, hooks, mcpServers, settings (no rules) | 1.1.90 |
| 21 | IDE path sharing: CLI docs never mention the IDE | upstream-watchlist.md:151 | unchanged | not re-fetched (overview page not reached); memory, settings, config-scope pages fetched here name no IDE | https://docs.qoder.com/cli/config-scope.md | no IDE mention in the fetched text (absence claim, summarized fetch) | 1.1.90 |
| 22 | Project settings/local settings apply only in a trusted cwd (context for rows 10-11) | docs settings.md:406 area | n/a | settings file locations unchanged: user `~/.qoder/settings.json`, project `<project>/.qoder/settings.json`, local `<project>/.qoder/settings.local.json` | https://docs.qoder.com/cli/settings.md | "Project-Level and Local Level configurations are only applied when the current working directory is trusted." | 1.1.90 |

## Verifier sign-off

none required — no code landed in this run.

## Routing

- #12 Support-dir exclusion: agentsMdExcludes still does not cover rules/ → (a) ledger or doc text fix
- #15 MCP env-ref expansion not documented in mcp-reference → (a) ledger or doc text fix
- #19 .agents/skills pool: not pool-capable; changelog-only evidence → (a) ledger or doc text fix
- #20 Plugin manifest .qoder-plugin/plugin.json; marketplace required keys name,... → (a) ledger or doc text fix
- #21 IDE path sharing: CLI docs never mention the IDE → (a) ledger or doc text fix
- #22 Project settings/local settings apply only in a trusted cwd (context for rows... → (a) ledger or doc text fix
- (c) security — .agents/upstream-watchlist.md:147 · expansion
- (c) security — src/install/vendor_qoder.rs:61 · trust-gate key (not a row)

## Issue drafts

none

## Friction

- dropped (unsourced): Plugin marketplace lookup order (`.qoder-plugin` then `.claude-plugin` then root) and `./` source rule, `${QODER_PLUGIN_ROOT}` placeholders — plugins-reference.md says nothing; row 154 rests on bundle reads (1.1.64), not re-readable here (no npm install allowed).
- dropped (unsourced): Hook surface rows (watchlist 250, 251) and `vendor_qoder.rs` hook stamps — no hooks docs page was in the listed Docs; not fetched.
- dropped (unsourced): Rows 45, 46, 60-62 (plugin logo/marketplace metadata) — mention Qoder among others, rest on recorded 2026-10-17 research; not re-fetched.
- dropped (unsourced): Row 218 private clone, row 88-style stamps — not applicable/not reached.
- not reached: upstream-watchlist.md:218 (private clone, Unverified)
- not reached: upstream-watchlist.md:45, 46, 60, 61, 62 (marketplace/logo rows naming Qoder)
- not reached: upstream-watchlist.md:250, 251 (hooks)
- not reached: docs statements in clients.md:228-280 (hooks gap) and mcp-servers.md:99/102 (timeout, cwd projection for Qoder)
- not reached: docs.qoder.com/cli/overview (IDE claim, row 21 only inferred)
- feed: docs.qoder.com/release-notes/qoder-cli 307-redirects to https://changelog.qoder.com/cli (domains.md feed URL is stale); quotes came through a summarizing WebFetch, not raw markdown.
- feed: newest CLI 1.1.90 (2026-11-16), cursor found yes

## Skill changes

Host caps concurrent subagents at 20; the researcher built its own claim list; replies are a short summary with the table in a file. Fixed in this run's chore(skills) commit.
