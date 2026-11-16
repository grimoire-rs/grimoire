# Research: upstream refresh — kilo (sweep)

## Metadata
**Date:** 2026-11-16 · **Domain:** vendors · **Depth:** sweep — checked 50d ago
**Triggered by:** /upstream-refresh
**Expires:** 2027-05-16

## Claims

| # | Claim | Ledger location | Old value | New value | Source URL | Verbatim quote | Vendor version |
|---|---|---|---|---|---|---|---|
| 1 | Global pool: Kilo discovers `~/.agents/skills` by default | .agents/upstream-watchlist.md:124; src/install/vendor_kilo.rs:22 | pool-capable, verified 2026-09-27 v7.8.1 | unchanged | https://github.com/Kilo-Org/kilocode/blob/v7.8.28/packages/kilo-docs/pages/customize/skills.md | "To share personal skills across projects, install them at `~/.agents/skills/<name>/SKILL.md`. Kilo discovers this user-level directory by default, without a `skills.paths` entry or a plugin to register the skills." | v7.8.28 |
| 2 | `KILO_DISABLE_EXTERNAL_SKILLS` keeps pool scan off (default false) | .agents/upstream-watchlist.md:124; docs clients.md:448 | scans `~/.agents/skills` unless flag set | unchanged (source: `if (!disableExternalSkills) { ... externalDirs.push(AGENTS_EXTERNAL_DIR)`; docs show `export KILO_DISABLE_EXTERNAL_SKILLS=true`) | https://github.com/Kilo-Org/kilocode/blob/v7.8.28/packages/opencode/src/skill/index.ts | "if (!disableExternalSkills) {" ... "externalDirs.push(AGENTS_EXTERNAL_DIR)" | v7.8.28 |
| 3 | Write root `~/.kilo`; `globalDirs()` returns `[~/.kilocode, ~/.kilo]` | .agents/upstream-watchlist.md:125 | verified 2026-09-27 v7.8.1 | unchanged | https://github.com/Kilo-Org/kilocode/blob/v7.8.28/packages/opencode/src/kilocode/paths.ts | "export function globalDirs(): string[] { return [path.join(home(), \".kilocode\"), path.join(home(), \".kilo\")] }" (doc comment: "legacy first, .kilo wins later") | v7.8.28 |
| 4 | Custom subagents: global `~/.config/kilo/agents/`, project `.kilo/agents/`, filename is agent name | .agents/upstream-watchlist.md:122,125; vendor_kilo.rs:36; docs clients.md:617 | verified 2026-09-27 v7.8.1 | unchanged | https://github.com/Kilo-Org/kilocode/blob/v7.8.28/packages/kilo-docs/pages/customize/custom-subagents.md | "**Global**: `~/.config/kilo/agents/`" / "**Project-specific**: `.kilo/agents/`" / "The **filename** (without `.md`) becomes the agent name." | v7.8.28 |
| 5 | `ConfigPaths.directories` puts `Global.Path.config` first, then walks `.kilocode`/`.kilo` up from project and from home (so `~/.kilo/agents` is also scanned, clients.md:619) | .agents/upstream-watchlist.md:122; docs clients.md:619 | verified 2026-09-27 | unchanged | https://github.com/Kilo-Org/kilocode/blob/v7.8.28/packages/opencode/src/config/paths.ts | "Global.Path.config," ... "targets: [\".kilocode\", \".kilo\"]" ... "start: Global.Path.home," | v7.8.28 |
| 6 | Agent frontmatter registry: `model`,`mode`,`temperature`,`top_p`,`steps`,`disable`,`hidden`,`color` lifted; `permission`, `variant` not in registry; `prompt` left out | .agents/upstream-watchlist.md:122; docs vendor-metadata.md:365-377 | verified 2026-09-27 | unchanged (schema still has `variant`, `permission`, `maxSteps` deprecated, `tools` deprecated; `color`: "Hex color code (e.g., #FF5733) or theme color (e.g., primary)"; `steps` PositiveInt; `hidden`: "only applies to mode: subagent") | https://github.com/Kilo-Org/kilocode/blob/v7.8.28/packages/core/src/v1/config/agent.ts | "mode: Schema.optional(Schema.Literals([\"subagent\", \"primary\", \"all\"]))" ; "description: \"Hex color code (e.g., #FF5733) or theme color (e.g., primary)\"" | v7.8.28 |
| 7 | Agent load builds `{ name, ...frontmatter, prompt: body }`; invalid agent skipped with warning | .agents/upstream-watchlist.md:122 | verified 2026-09-27 | unchanged | https://github.com/Kilo-Org/kilocode/blob/v7.8.28/packages/opencode/src/config/agent.ts | "const config = { name, ...md.data, prompt, }" ; "a rejected substitution must skip only this agent with a warning, not fail the whole config load" | v7.8.28 |
| 8 | `.kilocode` legacy dir read-only fallback, `.kilo/` wins | .agents/upstream-watchlist.md:126 | verified 2026-09-27 | unchanged (docs text drifted slightly: now "last one wins") | https://github.com/Kilo-Org/kilocode/blob/v7.8.28/packages/opencode/src/kilocode/docs/migration.md | "Canonical `.kilo/` skills take precedence over legacy `.kilocode/` skills at the same project level." | v7.8.28 |
| 9 | MCP env-ref form `{env:VAR}` (MCP declined) | .agents/upstream-watchlist.md:127; vendor_kilo.rs:49 | verified 2026-09-27 | unchanged | https://github.com/Kilo-Org/kilocode/blob/v7.8.28/packages/kilo-docs/pages/automate/mcp/using-in-cli.md | "Use `{env:VARIABLE_NAME}` syntax in config files to reference environment variables:" | v7.8.28 |
| 10 | opencode lineage: separate client, Kilo adopts OpenCode releases | .agents/upstream-watchlist.md:134 | v7.7.9 "Adopt OpenCode v1.18.19 through v1.18.20" | unchanged (release body still carries it) | https://github.com/Kilo-Org/kilocode/releases/tag/v7.7.9 | "Adopt OpenCode v1.18.19 through v1.18.20 improvements" | v7.7.9 |
| 11 | Settings key `/model` string | vendor_kilo.rs:155; upstream-watchlist.md:304; settings.md:792 | string scalar global | unchanged | https://app.kilo.ai/config.json | "Model to use in the format of provider/model, eg anthropic/claude-2" | schema live 2026-11-16 |
| 12 | Settings key `/small_model` string | vendor_kilo.rs:156; settings.md:793 | string | unchanged | https://app.kilo.ai/config.json | "Small model to use for tasks like title generation in the format of provider/model" | schema live |
| 13 | Settings key `/default_agent` string | vendor_kilo.rs:157; settings.md:794 | string | unchanged | https://app.kilo.ai/config.json | "Default primary agent to use when no session agent is selected" | schema live |
| 14 | `/reasoning_display` enum expanded, preview, headline | vendor_kilo.rs:159; settings.md:795 | enum | unchanged | https://app.kilo.ai/config.json | "Controls how reasoning blocks are displayed in the VS Code chat UI" (enum: expanded, preview, headline) | schema live |
| 15 | `/terminal_command_display` enum expanded, collapsed | vendor_kilo.rs:164; settings.md:796 | enum | unchanged | https://app.kilo.ai/config.json | "Controls whether terminal command blocks are expanded or collapsed by default in the VS Code chat UI" | schema live |
| 16 | `/code_edit_display` enum expanded, collapsed | vendor_kilo.rs:169; settings.md:797 | enum | unchanged | https://app.kilo.ai/config.json | "Controls whether code edit and diff blocks are expanded or collapsed by default in the VS Code chat UI" | schema live |
| 17 | `/web_search` bool | vendor_kilo.rs:174; settings.md:798 | bool | unchanged | https://app.kilo.ai/config.json | "Make web search available to models from all providers (default: false)" | schema live |
| 18 | `/hide_prompt_training_models` bool | vendor_kilo.rs:175; settings.md:799 | bool | unchanged | https://app.kilo.ai/config.json | "Hide Kilo Gateway models that may train on your prompts from model listings" | schema live |
| 19 | `/privacy_mode` bool | vendor_kilo.rs:176; settings.md:800 | bool | unchanged | https://app.kilo.ai/config.json | "Blur personally identifiable information (account email, balance, team name, etc.) in the TUI and require confirmation before showing profile details" | schema live |
| 20 | Settings key list = 9 rows; re-check trigger: runtime reads `update` or `snapshots`, or schema changes | .agents/upstream-watchlist.md:304 | 9 rows, verified 2026-11-05 at 7.8.18 | schema HAS changed since: new top-level scalar keys `memory_model` (string), `auto_collapse_reasoning`, `auto_expand_history`, `subagent_depth`, `timezone`, `warming`; `update` (enum disable/notify/auto) and `snapshots` (bool) still in schema; whether the runtime reads `update`/`snapshots` not determined. Trigger "schema changes" has fired. | https://app.kilo.ai/config.json | memory_model: "Model for automatic project memory saves in the format of provider/model. If unset or unavailable, memory uses the session model." ; update: "Disable updates, notify when one is available, or install updates automatically" | schema live; memory_model introduced in 7.8.x per PR #14781 |
| 21 | Settings UI clears a key from every config file | .agents/upstream-watchlist.md:315; settings.md:928 | verified 2026-11-05 at 7.8.18 | unchanged | https://kilo.ai/docs/llms.txt (Managing Settings) | "Clearing a setting in the Settings UI (for example, setting a model back to \"Not set\") removes it from every config file that contains it." | docs live 2026-11-16 |
| 22 | Project file and sibling files override / deep-merge | .agents/upstream-watchlist.md:316; settings.md:998; vendor_kilo.rs:54 | verified 2026-11-05 | unchanged | https://kilo.ai/docs/llms.txt (Managing Settings) | "Kilo reads JSONC config from a **global** location (`~/.config/kilo/kilo.jsonc`) and from your **project** (`kilo.jsonc`, or `.kilo/kilo.jsonc`)." "If `kilo.json` or the legacy `opencode.json`, `opencode.jsonc`, or `config.json` files exist in the same locations, Kilo reads and deep-merges them as well." | docs live |
| 23 | Expansion: `{env:`/`{file:` expanded in config strings; refusal in settings values stays correct | .agents/upstream-watchlist.md:317; settings.md:192; vendor_kilo.rs:635 | verified 2026-11-05 | unchanged for settings (global config is trusted, expands both). Docs wording now also: project config cannot use `{env:}` (ignored, warns) and `{file:}` is confined to the project root. | https://kilo.ai/docs/llms.txt (Managing Settings) | "`{env:VAR}` (and `{file:...}`) references are resolved **only** in trusted config: your global config (`~/.config/kilo`), a config passed via `KILO_CONFIG` / `KILO_CONFIG_CONTENT`, or organization/MDM-managed config." | docs live |
| 24 | Trust gates: no key that disables a trust gate is proposed as a row | vendor_kilo.rs:155 (all 9 rows) | none of the 9 is a trust key | unchanged; schema scan found no trust/consent key among nine; none newly proposed | https://app.kilo.ai/config.json | n/a: absence claim; keys listed above carry no trust semantics | schema live |
| 25 | NEW (no row): agent markdown BODY expands `{env:VAR}` / `{file:...}` when the agent dir is trusted (global `~/.config/kilo/agents`); project agents untrusted | none (uncovered); grim renders agent body verbatim, vendor_kilo.rs:36 | not recorded | body is substituted via ConfigVariable before use as prompt; present already at v7.8.1 (4 `substitute` hits), so not new upstream, but never recorded | https://github.com/Kilo-Org/kilocode/blob/v7.8.28/packages/opencode/src/config/agent.ts | "const prompt = await ConfigVariable.substitute({ text: md.content.trim(), type: \"virtual\", ... trusted, fileScope, markdown: true," and "Project agents are untrusted (no {env:}, {file:} confined to fileScope.root)" | v7.8.28 |
| 26 | Hook kind: Kilo takes a JS/TS plugin module, no config-level hook surface | .agents/upstream-watchlist.md:236 | declined | unchanged (schema still has `plugin`/`plugins`, no hooks key) | https://app.kilo.ai/config.json | schema top-level keys include `plugin`, `plugins`; no `hooks` key | schema live |
| 27 | Rules: still a flat `instructions` array, no per-file scoping | .agents/upstream-watchlist.md:120; docs clients.md:562 | verified 2026-09-27 | unchanged | https://app.kilo.ai/config.json | schema top-level key `instructions` present; no per-file rules directory key | schema live |

## Verifier sign-off

Signed off 2026-11-16 by three read-only sonnet verifiers (upstream re-fetched, grim code read) and by the implementing workers; one line per code-driving claim.

- #20 `/memory_model` (string): confirmed against the live schema; landed in `58754de8` as a row.
- #25 global agent body expands `{env:` and `{file:` (project agents do not): confirmed, `ConfigVariable.substitute` in `agent.ts` at v7.8.28; landed in `6c6d3b9f` as a decline with a warning at global scope. Narrowed: global scope only, and an agent already installed stays installed.

## Routing

- #2 KILO_DISABLE_EXTERNAL_SKILLS keeps pool scan off (default false) → (a) ledger or doc text fix
- #6 Agent frontmatter registry:... → (a) ledger or doc text fix
- #8 .kilocode legacy dir read-only fallback, .kilo/ wins → (a) ledger or doc text fix
- #10 opencode lineage: separate client, Kilo adopts OpenCode releases → (a) ledger or doc text fix
- #20 Settings key list = 9 rows; re-check trigger: runtime reads update or... → (b) `/memory_model` landed in `58754de8`
- #24 Trust gates: no key that disables a trust gate is proposed as a row → (a) ledger or doc text fix
- #26 Hook kind: Kilo takes a JS/TS plugin module, no config-level hook surface → (a) ledger or doc text fix
- (c) security — src/install/vendor_kilo.rs:36 · expansion
- #25 agent body expansion (new row) → (b) landed in `6c6d3b9f`: a global agent whose body holds `{env:` or `{file:` is skipped with a warning

## Issue drafts

none

## Friction

- dropped (unsourced): upstream-watchlist.md:126 "2026-07-31 EOL date" for `.kilocode`: no primary source found in the v7.8.28 migration.md or docs grep (same as prior pass).
- dropped (unsourced): vendor_kilo.rs:6 "verified 2026-07-27" original stamp: superseded by later stamps, not separately re-fetched.
- dropped (unsourced): Rows 134 `v7.7.9` quote taken from release body via `gh api`, which is the vendor repo; counted sourced.
- dropped (unsourced): docs clients.md:612 "`.kilocode/` is deprecated upstream": migration.md calls it "legacy"; the word "deprecated" not found verbatim. Left unsourced.
- not reached: upstream-watchlist.md:102/104 section-level text (Wave-2 declines for Cline/Droid/Goose/Warp/OpenClaw: not Kilo-specific beyond rows 120-127)
- not reached: upstream-watchlist.md:255 (OpenCode-only row naming Kilo only as a shape analogue)
- not reached: docs mcp-servers.md:239 (Kilo MCP decline reasons)
- not reached: docs vendor-metadata.md:471/501 skills-root rows (covered indirectly by rows 3, 8)
- feed: newest v7.8.27 stable (2026-11-05), cursor found yes

## Skill changes

Host caps concurrent subagents at 20; the researcher built its own claim list; replies are a short summary with the table in a file. Fixed in this run's chore(skills) commit.
