# Research: upstream refresh — junie (sweep)

## Metadata
**Date:** 2026-11-16 · **Domain:** vendors · **Depth:** sweep — checked 50d ago
**Triggered by:** /upstream-refresh
**Expires:** 2027-05-16

## Claims

| # | Claim | Ledger location | Old value | New value | Source URL | Verbatim quote | Vendor version |
|---|---|---|---|---|---|---|---|
| 1 | Agent kind: project `.junie/agents/<n>.md`, global `~/.junie/agents/<n>.md`; also reads `.agents/` and `~/.agents/` | .agents/upstream-watchlist.md:69; src/install/vendor_junie.rs:36-38 | as listed | unchanged | https://junie.jetbrains.com/docs/junie-cli-subagents.html | "Project scope: <projectRoot>/.junie/agents/ . Project scope: <projectRoot>/.agents/ . User scope: ~/.junie/agents/ on macOS/Linux or %USERPROFILE%\.junie\agents\ on Windows. User scope: ~/.agents/ on macOS/Linux" | doc dated 16 Nov 2026 (release 26.11.x) |
| 2 | Agent frontmatter: `name` pattern `[a-z][a-z0-9_-]*`, filename if omitted; `description` required; `tools` List<String>; `permissionMode` enum default/acceptEdits/dontAsk/bypassPermissions/plan; `maxTurns` positive Int; also `disallowedTools`, `mcpServers`, `skills`, `allowPromptArgument` | watchlist:69; vendor-metadata.md:391-400 | as listed | unchanged | https://junie.jetbrains.com/docs/junie-cli-subagents.html | "Allowed values: default , acceptEdits , dontAsk , bypassPermissions , plan . Defaults to default ." and "Must match: [a-z][a-z0-9_-]* (lowercase letters, digits, hyphens, underscores)." and "Must be a positive integer." | 26.11.x |
| 3 | `reasoningLevel` values low/medium/high, model-dependent | watchlist:69; vendor-metadata.md:392 | `reasoningLevel` only | additive: the doc now also accepts `effort` as an alias of `reasoningLevel`, and `effort` wins if both are present. grim does not lift `effort`; values text unchanged | https://junie.jetbrains.com/docs/junie-cli-subagents.html | "The effort key is accepted as an alias (and takes precedence if both are present). In most cases, use values such as low , medium , or high , but supported values can vary by model" | 26.11.x |
| 4 | Junie imports agents from `.cursor/agents/`, `.claude/agents/`, `.codex/agents/` | watchlist:69; vendor_junie.rs:44-46; clients.md:393 | as listed | unchanged | https://junie.jetbrains.com/docs/junie-cli-subagents.html | "Junie CLI detects such files when opening the project and suggests importing them into Junie's .junie/agents/ directory automatically." | 26.11.x |
| 5 | Subagent EAP sentence only under the model-selection setting; agents GA | watchlist:69; vendor_junie.rs:352 | EAP only on model-selection setting | unchanged | https://junie.jetbrains.com/docs/junie-cli-subagents.html | "This setting is currently in the Early Access Program . To try it, install the Early Access version of Junie CLI." (under "Configure subagent usage") | 26.11.x |
| 6 | Extension/plugin discovery of `agents/` inside a Claude-format plugin (marked Unverified) | watchlist:69 | unverified | partly resolved: an extension is a bundle that can carry Subagents, and Junie reads both manifest formats. Where inside the bundle agents live is still not stated. Stays unverified | https://junie.jetbrains.com/docs/junie-cli-extensions.html | "A single extension can package any combination of: Agent skills MCP servers Subagents Custom slash commands Guidelines Hooks" and "The Claude plugin format at .claude-plugin/marketplace.json ." | 26.11.x |
| 7 | Rule scoping: `.junie/rules/*.md` flat, no per-file activation key | watchlist:70; vendor_junie.rs:13-33; vendor-metadata.md:548 | Degraded, `paths` dropped | unchanged | https://junie.jetbrains.com/docs/environment-variables.html | ".junie/rules/*.md . All Markdown files in the rules directory." | 26.11.x |
| 8 | Guidelines discovery order. Wording changed: `.junie/AGENTS.md` is exclusive when present; otherwise root AGENTS.md + playbook + rules are concatenated; legacy file only when none exist | watchlist:70, :74 | rules sit at "step 4", legacy "step 5" | order restated, no per-file key. Text fix only | https://junie.jetbrains.com/docs/environment-variables.html and https://junie.jetbrains.com/docs/guidelines-and-memory.html | "If this file exists, it is used exclusively; no other guidelines files are combined with it." / ".junie/guidelines.md . Legacy guidelines file, used only when none of the above exist (still supported)." | 26.11.x |
| 9 | Global rules dir: none; only `~/.junie/AGENTS.md` global guidelines | watchlist:71; vendor_junie.rs:30-33; clients.md:378 | no `~/.junie/rules/` | unchanged | https://junie.jetbrains.com/docs/guidelines-and-memory.html | "Junie CLI also supports global guidelines from ~/.junie/AGENTS.md . On Windows, the global guidelines path is %USERPROFILE%\.junie\AGENTS.md ." | 26.11.x |
| 10 | MCP env interpolation undocumented; only literal `env` | watchlist:72; vendor_junie.rs:34-35; mcp-servers.md:224 | ref-bearing descriptors skipped | unchanged. Page also now documents an optional per-server `timeout` (ms, 1000-3600000) that grim does not write | https://junie.jetbrains.com/docs/junie-cli-mcp-configuration.html | "\"env\": { \"ENV_VAR\": \"value\" }" and "set the optional timeout field on that server in milliseconds" | 26.11.x |
| 11 | MCP paths `.junie/mcp/mcp.json`, `~/.junie/mcp/mcp.json`, key `mcpServers` | vendor_junie.rs:34; mcp-servers.md:224 | as listed | unchanged | https://junie.jetbrains.com/docs/junie-cli-mcp-configuration.html | "Project scope : .junie/mcp/mcp.json at the root of your project. User scope : ~/.junie/mcp/mcp.json on your machine." | 26.11.x |
| 12 | `JUNIE_*_LOCATIONS` additive, `*_DEFAULT_LOCATIONS` default true; AGENT/MCP/SKILL members, no rules member | watchlist:73; vendor_junie.rs:48-49; vendor-metadata.md:490 | not honored | unchanged. New members exist: `JUNIE_COMMAND_LOCATIONS`, `JUNIE_COMMAND_DEFAULT_LOCATIONS`, `JUNIE_MODEL_*`, `JUNIE_CONFIG_*`, `JUNIE_GUIDELINES_FILENAME`, `JUNIE_EXTENSIONS_DEFAULT_LOCATION` | https://junie.jetbrains.com/docs/environment-variables.html | "JUNIE_SKILL_LOCATIONS --skill-location Additional paths where Junie should search for agent skills." / "JUNIE_SKILL_DEFAULT_LOCATIONS ... Enable or disable default skill locations (per-user and per-project). Defaults to true ." | 26.11.x |
| 13 | `JUNIE_HOME` replaces `~/.junie` | watchlist:171; vendor-metadata.md:490; mcp-servers.md:224 | honored read-only | unchanged | https://junie.jetbrains.com/docs/environment-variables.html | "JUNIE_HOME - Home directory for Junie CLI. Overrides the default ~/.junie ." | 26.11.x |
| 14 | Legacy `guidelines/` folder: no per-file key; not written | watchlist:74 | not written | unchanged | https://junie.jetbrains.com/docs/guidelines-and-memory.html | ".junie/guidelines.md file or .junie/guidelines/ folder – Junie's legacy format for guidelines (still supported)." | 26.11.x |
| 15 | Skills `.junie/skills/<name>/`, `~/.junie/skills/<name>/`; also `.agents/skills/` | vendor_junie.rs:10-11; vendor-metadata.md:466,490 | as listed | unchanged | https://junie.jetbrains.com/docs/agent-skills.html | "Project scope : <projectRoot>/.junie/skills/<skill-name>/ ." and "User scope : ~/.junie/skills/<skill-name>/ on macOS/Linux" | 26.11.x |
| 16 | Settings re-check trigger: Junie documents config.json vs settings.json precedence, or `brave` and permission keys inside `config.json` | domains.md:110 | not fired | FIRED in part: `brave` is a listed `config.json` field (unclear if new since 2026-09-27). Precedence still undocumented, and the page says settings.json is a separate channel. No permission key in config.json | https://junie.jetbrains.com/docs/junie-cli-configuration.html | "brave Enables brave mode by default." and "There is no single global precedence across both channels: settings such as the model, provider, brave mode, and effort each resolve through their own consumer path" | 26.11.x |
| 17 | Hook kind declined for Junie: "splices shell-command hooks into a shared config file" | watchlist:234 | declined, grim-side schedule | context changed: Junie hooks (EAP) live under `hooks` in `~/.junie/config.json` or a `--config-location` file. Project `.junie/config.json` hooks are ignored by design, so no project-scope splice target exists | https://junie.jetbrains.com/docs/junie-cli-hooks.html | "Project-local hooks from <project-root>/.junie/config.json are ignored by default for safety." and "This feature is currently in the Early Access Program ." | 26.11.x |
| 18 | Doc: `.junie/rules/` project only, no global rules | clients.md:364-380 | as listed | unchanged (see rows 7, 9) | https://junie.jetbrains.com/docs/guidelines-and-memory.html | see row 9 | 26.11.x |

## Verifier sign-off

none required — no code landed in this run.

## Routing

- #3 reasoningLevel values low/medium/high, model-dependent → (b) additive renderer or metadata support — reported, not implemented in this run
- #6 Extension/plugin discovery of agents/ inside a Claude-format plugin (marked... → (a) ledger or doc text fix
- #8 Guidelines discovery order. Wording changed: .junie/AGENTS.md is exclusive when... → (a) ledger or doc text fix
- #10 MCP env interpolation undocumented; only literal env → (b) additive renderer or metadata support — reported, not implemented in this run
- #12 JUNIE_*_LOCATIONS additive, *_DEFAULT_LOCATIONS default true; AGENT/MCP/SKILL... → (a) ledger or doc text fix
- #16 Settings re-check trigger: Junie documents config.json vs settings.json... → (c) issue draft
- #17 Hook kind declined for Junie: "splices shell-command hooks into a shared config... → (c) issue draft
- #18 Doc: .junie/rules/ project only, no global rules → (a) ledger or doc text fix
- (c) security — .agents/upstream-watchlist.md (junie rows) · upstream trust gate

## Issue drafts

### Junie settings: config.json now lists brave, re-check trigger fired in part

Junie's CLI configuration page lists `brave` as a `config.json` field and says settings.json is a separate channel; precedence between the two is still undocumented and no permission key appears in config.json. It is unclear whether `brave` is new since 2026-09-27. Source: https://junie.jetbrains.com/docs/junie-cli-configuration.html. The re-check trigger is recorded at `.claude/skills/upstream-refresh/references/domains.md:110` (ledger row in the claims table).

Labels: documentation

### Junie hooks: EAP hooks live in ~/.junie/config.json, no project-scope splice target

Junie hooks (EAP) live under `hooks` in `~/.junie/config.json` or a `--config-location` file; project `.junie/config.json` hooks are ignored by design. Source: https://junie.jetbrains.com/docs/junie-cli-hooks.html. grim declines the hook kind for Junie on the premise of splicing shell-command hooks into a shared config file (`.agents/upstream-watchlist.md:234`); the new fact leaves no project-scope target.

Labels: enhancement

## Friction

- dropped (unsourced): Watchlist rows 70/74 earlier-dated claims (JetBrains/junie-guidelines README, YouTrack JUNIE-1829 and JUNIE-84) — not re-fetched, historical evidence only.
- dropped (unsourced): Whether `effort` alias and `brave` in config.json are new since 2026-09-27 — whats-new lists only releases from 2026-10-28, GH release bodies are empty, so no dated changelog for them.
- not reached: vendor-metadata.md:130/148/153/431 and clients.md:59 matrix cell (symbol pattern: they restate rows 1-7, no separate upstream fact).
- feed: whats-new page reaches back only to 2026-10-28, so 2026-09-27 to 2026-10-27 has no heading coverage; GH tag bodies are empty.
- feed: newest GH release 4136.1 (2026-11-16), cursor found yes

## Skill changes

Host caps concurrent subagents at 20; the researcher built its own claim list; replies are a short summary with the table in a file. Fixed in this run's chore(skills) commit.
