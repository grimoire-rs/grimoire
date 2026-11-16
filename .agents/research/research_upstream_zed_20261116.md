# Research: upstream refresh — zed (sweep)

## Metadata
**Date:** 2026-11-16 · **Domain:** vendors · **Depth:** sweep — checked 50d ago
**Triggered by:** /upstream-refresh
**Expires:** 2027-05-16

## Claims

| # | Claim | Ledger location | Old value | New value | Source URL | Verbatim quote | Vendor version |
|---|---|---|---|---|---|---|---|
| 1 | Rules: `RULES_FILE_NAMES` = `.rules`, `.cursorrules`, `.windsurfrules`, `.clinerules`, `.github/copilot-instructions.md`, `AGENT.md`, `AGENTS.md`, `CLAUDE.md`, `GEMINI.md`, first match wins, no scoping (declined) | .agents/upstream-watchlist.md:80; src/install/vendor_zed.rs:9-10 | verified 2026-09-27 v1.21.0, 9 names | unchanged (list byte-identical v1.21.0 vs v1.28.1) | https://github.com/zed-industries/zed/blob/v1.28.1/crates/prompt_store/src/prompts.rs | `pub const RULES_FILE_NAMES: &[&str] = &[ ".rules", ".cursorrules", ".windsurfrules", ".clinerules", ".github/copilot-instructions.md", "AGENT.md", "AGENTS.md", "CLAUDE.md", "GEMINI.md", ];` | v1.28.1 |
| 2 | MCP env refs: no `${VAR}` expansion in `context_servers` env/headers; ref-bearing descriptors skipped | .agents/upstream-watchlist.md:81; src/install/vendor_zed.rs:14-16; docs clients.md:491-495 | verified 2026-09-27 v1.21.0 | unchanged. Docs show literal `"env": {}` and `"headers": { "Authorization": "Bearer <token>" }`, no expansion text in the MCP page. PR #60457 still closed, merged=false (closed 2026-07-09). | https://zed.dev/docs/ai/mcp.md ; https://github.com/zed-industries/zed/pull/60457 | `"remote-mcp-server": { "url": "https://example.com/mcp", "headers": { "Authorization": "Bearer <token>" } }` | v1.28.1 docs |
| 3 | MCP `oauth` block: HTTP `context_servers` carry `oauth: OAuthClientSettings { client_id (required), client_secret? }`; only `client_id` projected | .agents/upstream-watchlist.md:82; docs mcp-servers.md:169, clients.md:156-157 | verified 2026-09-27 v1.21.0 | unchanged in source. Docs MCP page still describes only the browser OAuth flow. | https://github.com/zed-industries/zed/blob/v1.28.1/crates/settings_content/src/project.rs | `pub struct OAuthClientSettings { /// The OAuth client ID obtained from out-of-band registration with the authorization server. pub client_id: String, ... pub client_secret: Option<String>,` | v1.28.1 |
| 4 | macOS `$XDG_CONFIG_HOME` root not honored; Linux/FreeBSD XDG, Windows `%APPDATA%\Zed`, macOS `~/.config/zed` | .agents/upstream-watchlist.md:85; src/install/vendor_zed.rs:225-232; docs vendor-metadata.md:493, mcp-servers.md:226 | verified 2026-09-27 v1.21.0 | unchanged in `config_dir()`. Detail not in ledger: Linux branch first checks `FLATPAK_XDG_CONFIG_HOME` (present in v1.21.0 too, 4 hits); a `CUSTOM_DATA_DIR` (set by a CLI flag, not an env var) overrides everything. | https://github.com/zed-industries/zed/blob/v1.28.1/crates/paths/src/paths.rs | `} else if cfg!(any(target_os = "linux", target_os = "freebsd")) { if let Ok(flatpak_xdg_config) = std::env::var("FLATPAK_XDG_CONFIG_HOME") { ... dirs::config_dir().expect("failed to determine XDG_CONFIG_HOME directory") } ... } else { home_dir().join(".config").join(APP_NAME_LOWERCASE) }` | v1.28.1 |
| 5 | Hook kind: no upstream lifecycle-hook surface (declined) | .agents/upstream-watchlist.md:237 | no surface | unchanged. `llms.txt` (240-line docs index) has zero "hook" hits; the only settings-page hits are direnv `shell_hook` and an unrelated "Zed lifecycle-related behavior" setting. Release notes v1.22.0..v1.28.1 mention only "worktree task hooks" (task runner, not agent lifecycle). | https://zed.dev/docs/llms.txt | (no hit; absence claim) v1.28.1 release note: `Fixed worktree task hooks running when a workspace was in restricted mode.` | v1.28.1 |
| 6 | Shared pool: Zed reads `~/.agents/skills` (global) and `<worktree>/.agents/skills` (project), flat only; no Zed-native skills dir | src/install/vendor_zed.rs:11-12; .agents/upstream-watchlist.md:17 (pool join); docs vendor-metadata.md:467,493 | verified 2026-07-19 / 2026-09-27 | unchanged, now documented first-hand on a dedicated Skills page. Also new: project skills load only from trusted worktrees; frontmatter is only `name`, `description`, `disable-model-invocation`; 50KB catalog budget; global wins lose to same-name project skill. | https://zed.dev/docs/ai/skills.md | `Zed loads skills from two locations: Global ~/.agents/skills/ ; Project-local <worktree>/.agents/skills/ ... Each skill is a direct child of the skills root. Nesting skills inside subfolders is not supported.` | v1.28.1 docs |
| 7 | Re-check trigger (domains.md:111): "Zed docs state one path per OS" has NOT fired | .claude/skills/upstream-refresh/references/domains.md:111 | macOS settings path contradicts across pages; Windows unstated | unchanged, trigger not fired. remote-development still gives macOS as `~/.zed/settings.json` while external-agents/configuring pages say `~/.config/zed/settings.json` is the macOS file; no page states the Windows settings.json path (key-bindings gives keymap only). | https://zed.dev/docs/remote-development.md ; https://zed.dev/docs/ai/external-agents.md | remote-development: `The local Zed settings (in ~/.zed/settings.json on macOS or ~/.config/zed/settings.json on Linux)`; external-agents: `~/.config/zed/settings.json, which is Zed's settings file on macOS and on Linux without a custom XDG_CONFIG_HOME. On Windows, or with a custom config directory, install from the registry ...` | v1.28.1 docs |
| 8 | No settings rows for zed; `agent.tool_permissions.default` is the first row if the trigger fires | domains.md:111; docs settings.md:135,185 | none | unchanged: `tool_permissions` exists but trigger (claim 7) not fired, so no row. Docs still say `agent.tool_permissions.default` controls approval. | https://zed.dev/docs/ai/tool-permissions.md | `In Zed v0.224.0 and above, tool approval is controlled by agent.tool_permissions.default.` | v1.28.1 docs |
| 9 | Docs "Zed agents run over ACP with no installable file format" (agents declined) | docs clients.md:491-495; vendor_zed.rs:12 | declined | unchanged. Docs skills page: `Zed Skills apply to the Zed Agent. External Agents ... may have their own native skills, prompts, or instruction systems.` No agent-file format found. | https://zed.dev/docs/ai/skills.md | `Zed Skills apply to the Zed Agent. External Agents and Terminal Threads may have their own native skills, prompts, or instruction systems.` | v1.28.1 docs |

## Verifier sign-off

none required — no code landed in this run.

## Routing

- #1 Rules: RULES_FILE_NAMES = .rules, .cursorrules, .windsurfrules, .clinerules,... → (a) ledger or doc text fix
- #2 MCP env refs: no ${VAR} expansion in context_servers env/headers; ref-bearing... → (a) ledger or doc text fix
- #3 MCP oauth block: HTTP context_servers carry oauth: OAuthClientSettings {... → (a) ledger or doc text fix
- #4 macOS $XDG_CONFIG_HOME root not honored; Linux/FreeBSD XDG, Windows... → (a) ledger or doc text fix
- #5 Hook kind: no upstream lifecycle-hook surface (declined) → (a) ledger or doc text fix
- #7 Re-check trigger (domains.md:111): "Zed docs state one path per OS" has NOT... → (a) ledger or doc text fix
- #8 No settings rows for zed; agent.tool_permissions.default is the first row if... → (a) ledger or doc text fix
- #9 Docs "Zed agents run over ACP with no installable file format" (agents declined) → (a) ledger or doc text fix
- (c) security — .agents/upstream-watchlist.md:17 · upstream trust gate

## Issue drafts

none

## Friction

- dropped (unsourced): Watchlist row 17 (OpenCode duplicate-name auto-join, lists Zed as pool writer): not a Zed fact beyond claim 6; not re-fetched.
- dropped (unsourced): Rules docs page (https://zed.dev/docs/ai/rules.md) fetched but returned a 131KB unrelated blob; rules claim rests on source (claim 1) only.
- dropped (unsourced): Discussion zed #56881 (keychain indirection open): API returned a Feature Requests discussion; state/answer not read. Claim 2 therefore rests on docs + closed PR only.
- dropped (unsourced): Zed ACP "no hook mechanism" beyond docs index absence: no primary statement of absence exists.
- not reached: `.agents/upstream-watchlist.md` per-row date for Zed MCP/rules rows' companion text `research_upstream_zed_20260927.md` (not opened).
- not reached: docs vendor-metadata.md:436 "No namespace at all — Kiro, Zed, Amp, Qoder" (Zed has no vendor frontmatter namespace): skills page confirms only `name`, `description`, `disable-model-invocation` as fields (supports it), not separately logged.
- feed: newest v1.28.1 (2026-11-11), cursor found yes

## Skill changes

Host caps concurrent subagents at 20; the researcher built its own claim list; replies are a short summary with the table in a file. Fixed in this run's chore(skills) commit.
