# Research: upstream refresh — other harnesses settings (deep, #270)

Harnesses: agents, Amp, Cline, Junie, Zed, OpenClaw, Goose (combined artifact, R-B).

## Metadata
**Date:** 2026-11-05 · **Domain:** vendor AI-config layouts · **Depth:** deep (settings row)
**Triggered by:** /hex-execute .agents/plans/plan_settings_follow_ups.md (R-B)
**Expires:** 2027-05-05

Fetch method: raw pages through `curl` (markdown or HTML, tags stripped); quotes are verbatim from the raw page, not a WebFetch summary.

## Claims

### agents
| # | Key | Value type and values | Merge | Scope | Category | Source URL | Verbatim quote | Page date / version |
|---|---|---|---|---|---|---|---|---|
| A1 | none | no settings file exists | n/a | n/a | n/a | `src/install/vendor_agents.rs` (repo) | "`agents` is not a product. It is the vendor-neutral install target"; trait default `settings_fields()` returns `&[]`, `settings_config_path()` returns `None` | repo HEAD |

### Amp
Settings reference: https://ampcode.com/docs/markdown/cli/settings (frontmatter `lastModified: 2026-11-02`). Every key below is from that page. Merge for all scalars below: "When the same setting appears in multiple places, workspace settings override user settings." (scalar, workspace over user). All scope: global file `~/.config/amp/settings.json[c]`; the project file `.amp/settings.json[c]` is also documented.

| # | Key | Value type and values | Merge | Scope | Category | Verbatim quote |
|---|---|---|---|---|---|---|
| M1 | `/amp.showCosts` | boolean, default true | scalar | global (project file exists) | behaviour default | "**Type:** `boolean`, **Default:** `true` Show cost information for threads in the CLI while working." |
| M2 | `/amp.notifications.enabled` | boolean, default true | scalar | global | behaviour default | "Play notification sounds when the agent completes a task or is blocked waiting for user input." |
| M3 | `/amp.terminal.copyOnSelect` | boolean, default true | scalar | global | behaviour default | "By default, the Amp TUI copies the selection to the clipboard." |
| M4 | `/amp.terminal.detailsExpandedByDefault` | boolean, default false | scalar | global | behaviour default | "Expand thinking and tool call details by default in the CLI transcript." |
| M5 | `/amp.thread.autoArchiveOnQuit` | boolean, default false | scalar | global | behaviour default | "Automatically archive open CLI threads when quitting Amp." |
| M6 | `/amp.updates.mode` | string enum `auto`, `warn`, `disabled`; default `auto` | scalar | global | behaviour default | "`\"warn\"` shows update notifications. `\"disabled\"` turns off update checks. `\"auto\"` installs updates automatically." |
| M7 | `/amp.git.commit.ampThread.enabled`, `/amp.git.commit.coauthor.enabled` | boolean, default true | scalar | global | behaviour default | "Add the Amp thread trailer to Git commits." / "Add Amp as a coauthor in Git commits." |
| M8 | `/amp.tools.disable` | array of strings (names, `*` globs), default `[]` | stated only for the managed-settings document ("Lists are combined with workspace entries first"); user vs workspace file array merge unstated | global | permission policy (arguably) | "Disable specific tools by name." |
| M9 | `/amp.mcpPermissions` | array of `{matches, action}` objects, first match wins | unstated for user vs workspace; order-sensitive | global | permission policy | "Amp applies the first matching rule." |
| M10 | model or mode key | none on the page | n/a | n/a | n/a | the key list has no model, effort or mode key |
| M11 | marketplace key | none on the page | n/a | n/a | n/a | no marketplace or plugin-source key on the page |
| M12 | `amp.permissions` | not on the page | n/a | n/a | n/a | page lists only `amp.mcpPermissions`; scoping found "permissions moved to the plugin API" (https://ampcode.com/news/tool-level-permissions), not re-read in depth |

File facts: "User settings: macOS/Linux `~/.config/amp/settings.json` or `~/.config/amp/settings.jsonc`; Windows `%USERPROFILE%\.config\amp\settings.json` or `.jsonc`". Workspace: "the nearest `.amp/settings.json` or `.amp/settings.jsonc`". Relocation: `--settings-file` flag only (no env var documented). Which file wins when both `.json` and `.jsonc` exist: unstated. Managed override file (not a target): Linux `/etc/ampcode/managed-settings.json`. All keys carry the `amp.` prefix as a literal dotted key name (flat, not nested): the pointer is `/amp.showCosts`.

### Cline
Sources: https://docs.cline.bot/cli/configuration.md (the page's own index fetch), https://docs.cline.bot/cli/cli-reference.md. No page date shown. Live: `cline` 3.0.74.

| # | Key | Value type | Merge | Scope | Category | Verbatim quote |
|---|---|---|---|---|---|---|
| C1 | none | no key documented for `global-settings.json` | n/a | n/a | n/a | "`global-settings.json  # Global settings`" and "Global provider settings, global settings, and MCP settings are stored under `~/.cline/data/settings/`." No key names, types or values anywhere on the two pages |
| C2 | path | `~/.cline/data/settings/global-settings.json`; JSON (the doc says nothing about comments) | n/a | global | n/a | "Use **global (`~/.cline/`)** for defaults shared across all Cline applications". Relocation: `CLINE_DIR` replaces `~/.cline/`, `CLINE_DATA_DIR` replaces `~/.cline/data/`, `--config <dir>` |
| C3 | project settings | `cline config` lists "Settings (global + workspace)" but no workspace settings file path is given (`.cline/` layout lists no settings file) | n/a | project | n/a | "From there, you can view/edit: Settings (global + workspace)" |

### Junie
Source: https://junie.jetbrains.com/docs/junie-cli-configuration.html (page footer date 30 October 2026), https://junie.jetbrains.com/docs/junie-cli-model-selection.html, https://junie.jetbrains.com/docs/action-allowlist-junie-cli.html.

| # | Key | Value type and values | Merge | Scope | Category | Verbatim quote |
|---|---|---|---|---|---|---|
| J1 | `/model` | string; ids come from `junie --help` or `/model` (open set) | config.json user vs project: project wins when trusted; but see J6 | global (project file documented) | behaviour default | "model Default model to use." |
| J2 | `/effort` | string, "for models that support it"; values not enumerated; "Requires model" | as J1 | global | behaviour default | "effort Default reasoning effort for the configured model . Requires model , which can come from another configuration file." |
| J3 | `/provider` | string enum `openai`, `anthropic`, `google`, `xai`, `openrouter`, `copilot`, `litellm` (the model-selection page lists them for the provider flag and env var) | as J1 | global | behaviour default | "provider Default BYOK provider. For supported provider values, see LLM providers ." |
| J4 | `/brave` | boolean; meaning not defined on the page | as J1 | global | permission policy (unclear) | "brave Enables brave mode by default." |
| J5 | `/hooks`, `/byok`, `/proxies`, `/*-locations` | other categories or credential-bearing | n/a | n/a | out of the three categories | the example shows a `byok` block holding provider API keys: never a settings-kind target |
| J6 | cross-channel precedence | n/a | **unverified for J1 to J4** | n/a | n/a | "User settings stored in ~/.junie/settings.json are a separate channel and are not part of this config.json merge. There is no single global precedence across both channels: settings such as the model, provider, brave mode, and effort each resolve through their own consumer path, so a value from config.json does not simply override, or get overridden by, a value from settings.json." |
| J7 | within config.json | precedence stated | scalar, project over user | n/a | n/a | "Command-line flags Project configuration from <project-root>/.junie/config.json when the CLI project is trusted User configuration from ~/.junie/config.json" |
| J8 | permission policy | lives in a separate file `~/.junie/allowlist.json` (five action types), not `config.json` | n/a | global | permission policy | "You can manually add or remove allowed commands by editing the ~/.junie/allowlist.json file." |

File facts: `~/.junie/config.json`, `<project-root>/.junie/config.json`, JSON; relocation by `--config-location` / `JUNIE_CONFIG_LOCATION` (additional files, not a relocation of the default); `JUNIE_CONFIG_DEFAULT_LOCATIONS` disables defaults.

### Zed
Sources: https://zed.dev/docs/ai/tool-permissions.md, https://zed.dev/docs/ai/external-agents.md, https://zed.dev/docs/reference/all-settings.md, https://zed.dev/docs/remote-development.md, https://zed.dev/docs/ai/agent-settings.md. No page dates shown.

| # | Key | Value type and values | Merge | Scope | Category | Verbatim quote |
|---|---|---|---|---|---|---|
| Z1 | `/agent/tool_permissions/default` | string enum `confirm` (default), `allow`, `deny`; from Zed v0.224.0 | scalar (project over user is implied by "override global settings" below; no per-key statement) | global (project file documented) | permission policy | "In Zed v0.224.0 and above, tool approval is controlled by `agent.tool_permissions.default`." / "`default` Fallback when no patterns match: `\"confirm\"` (default), `\"allow\"`, or `\"deny\"`" |
| Z2 | `/agent/tool_permissions/tools/<tool>/{always_allow,always_deny,always_confirm}` | arrays of `{pattern, case_sensitive?}` objects (Rust regex) | unstated across user and project files; precedence between rule kinds is stated, merge of the arrays is not | n/a | permission policy | "Rule Precedence ... `always_deny`: Blocks matching actions ..." |
| Z3 | `/agent/default_model`, `/agent/subagent_model` | object `{provider, model, effort?, enable_thinking?, speed?}`; provider and effort values are open or model-specific | atomic object, merge unstated | n/a | behaviour default | "Choose an `effort` value supported by the selected model." |
| Z4 | marketplace key | none (`auto_install_extensions` is an extension auto-install map, not a marketplace source) | n/a | n/a | n/a | no marketplace-source key found |
| Z5 | settings file path | `~/.config/zed/settings.json` on macOS and Linux without custom `XDG_CONFIG_HOME`; Windows unstated; JSONC (examples carry comments) | n/a | global | n/a | "This command always writes to `~/.config/zed/settings.json`, which is Zed's settings file on macOS and on Linux without a custom `XDG_CONFIG_HOME`. On Windows, or with a custom config directory, ..." CONTRADICTION: remote-development.md says "(in `~/.zed/settings.json` on macOS or `~/.config/zed/settings.json` on Linux)" |
| Z6 | project merge | project file `.zed/settings.json` | "project settings are merged on top of user settings" (stated beside one key); migration page: "add a `.zed/settings.json` file inside a project to override global settings" | project | n/a | as quoted |

### OpenClaw
Sources: https://docs.openclaw.ai/gateway/configuration (page read; HTML), CLI `openclaw` 2026.10.10 (npm).

| # | Key | Value type | Merge | Scope | Category | Verbatim quote |
|---|---|---|---|---|---|---|
| O1 | file | `~/.openclaw/openclaw.json`; **JSON5** | n/a | global | n/a | "OpenClaw reads an optional **JSON5** config from `~/.openclaw/openclaw.json`." (tooltip: "JSON5 supports comments and trailing commas") |
| O2 | keys | not collected: format rules the harness out | n/a | n/a | n/a | n/a |

### Goose
Source: https://goose-docs.ai/docs/guides/config-files (no page date shown).

| # | Key | Value type | Merge | Scope | Category | Verbatim quote |
|---|---|---|---|---|---|---|
| G1 | file | `~/.config/goose/config.yaml` (macOS/Linux), `%APPDATA%\Block\goose\config\config.yaml`; **YAML** | n/a | global | n/a | "goose uses YAML configuration files to manage settings and extensions. The primary config file is located at: macOS/Linux: ~/.config/goose/config.yaml" |
| G2 | candidate keys `GOOSE_MODE`, `GOOSE_TEMPERATURE`, `GOOSE_TOOLSHIM`, `active_provider`, `providers.*` | listed in the docs | env over config file over defaults | global | behaviour default | "Settings are applied in the following order of precedence: Environment variables (highest priority) Config file settings Default values (lowest priority)". Not collected further: the format rules the harness out |

## Verifier sign-off
none required

## Live CLI check

### agents
not run: no binary and no settings file; the harness is a directory convention.

### Amp
`npm install --ignore-scripts --prefix <scratch> @sourcegraph/amp`, then `env -i HOME=<scratch> ... amp --version` and `amp config --help`: **not run**. The package ships the native binary through a postinstall step, which `--ignore-scripts` forbids ("Error: Amp native binary not installed"). No settings validation verb found.

### Cline
`env -i HOME=<scratch> TMPDIR=<scratch> XDG_CONFIG_HOME=<scratch> PATH=<scratch bin>:/usr/bin:/bin timeout 60 cline --version` (3.0.74), `cline config --help`, `cline config --json` (stdin from /dev/null): **ran and passed** as a listing. The verbs list workflows, rules, skills, hooks, agents, plugins, mcp and tools; they print no settings key and validate nothing, so no value run or bad-value control is possible for `global-settings.json`: settings check **skip** (the verb never reads settings keys).

### Junie
`npm install --ignore-scripts @jetbrains/junie`, `junie --version`: **not run**. The npm shim needs a postinstall-installed binary ("Shim not found"), forbidden under `--ignore-scripts`.

### Zed
not run: a GUI application, no npm package, no credential-free settings validation verb.

### OpenClaw
`env -i HOME=<scratch> ... openclaw --version` (2026.10.10), `openclaw config file` (printed `<scratch HOME>/.openclaw/openclaw.json`), `openclaw config schema`, `openclaw config validate`. A scratch `openclaw.json` containing a `//` comment, unquoted keys and trailing commas with `gateway.port: 19001`: **ran and passed** (`Config valid`). Bad-value control, `gateway.port: "notanumber"` in the same file: **ran and failed as expected** (`expected number, received string`). This confirms the file is parsed as JSON5 and that the verb reads it. (Node copied into the scratch bin for the shebang.)

### Goose
not run: not distributed through npm; a binary download was out of scope for a harness that has no rows by format.

## Routing

### agents
- Harness: no rows: no settings file, a directory convention (`settings_fields()` empty, `settings_config_path()` `None`) · re-check trigger: a vendor-neutral settings file convention appears in the agentskills or AGENTS.md specification
- Settings file: none
- Per key: none

### Amp
- Harness: rows (proposed, the orchestrator decides whether UI toggles count as "behaviour defaults"; the scoping artifact judged none in the three categories) · re-check trigger for the rest: Amp documents a model or mode key, or a marketplace key, in its settings reference
- Settings file: `~/.config/amp/settings.json` or `settings.jsonc` (Linux, macOS), `%USERPROFILE%\.config\amp\` (Windows) · JSONC · relocation: the settings page names only `--settings-file`. Corrected 2026-11-05 by the closing-review fix pass, which first read "no env var": `AMP_SETTINGS_FILE` stays contested and not honored (watchlist `$AMP_SETTINGS_FILE` row). The shipped linux-x64 binary `0.0.1793887133-gbf8b08` carries its help line "Set settings file path (can also use --settings-file, default: …)" (string read from the npm package, integrity checked, not run). Which file wins when both exist: unstated, so a splice must pick the existing one. Project scope: needs an ADR decision (`.amp/settings.json[c]` documented)
- Variable expansion: none for the eight rows (added 2026-11-05 by the closing-review fix pass). The [MCP page](https://ampcode.com/docs/markdown/customize/mcp) (lastModified 2026-11-04) documents `${VAR_NAME}` for `amp.mcpServers` only: "In configuration files, set `amp.mcpServers` and use `${VAR_NAME}` syntax for environment variables". The settings page (lastModified 2026-11-02) names no expansion for any other key. The eight rows are seven Booleans and one closed enum, so no delivered value can carry a token, and no build refusal is needed. Re-check if a string row is added.
- Per key: `/amp.showCosts` → row (bool, scalar, global) · `/amp.notifications.enabled` → row (bool, scalar, global) · `/amp.terminal.copyOnSelect` → row (bool, scalar, global) · `/amp.terminal.detailsExpandedByDefault` → row (bool, scalar, global) · `/amp.thread.autoArchiveOnQuit` → row (bool, scalar, global) · `/amp.updates.mode` → row (enum `auto`,`warn`,`disabled`, scalar, global) · `/amp.git.commit.ampThread.enabled` and `/amp.git.commit.coauthor.enabled` → row (bool, scalar, global; these change attribution, lower value) · `/amp.tools.disable` → off: array merge across user and workspace unstated · `/amp.mcpPermissions` → off: ordered first-match array, merge unstated · model or mode key → off: not documented · `amp.permissions` → off: not on the settings page · marketplace → off: no such key

### Cline
- Harness: no rows: no key documented for `global-settings.json` · re-check trigger: Cline docs list key names, types and values for `global-settings.json` (or a `cline config settings` verb prints them)
- Settings file: `~/.cline/data/settings/global-settings.json` · JSON · relocation `CLINE_DIR`, `CLINE_DATA_DIR`, `--config`. Project scope: needs an ADR decision (a workspace settings view exists, no file path documented)
- Per key: none

### Junie
- Harness: no rows: the page states that `model`, `provider`, `brave` and `effort` resolve through consumer paths with no single precedence across `config.json` and `~/.junie/settings.json`, so the effective merge is unverified; `effort` values are open; `brave` is undefined; permissions live in a separate `allowlist.json` · re-check trigger: Junie documents per-key precedence between `config.json` and `settings.json`, or documents `brave`/permission keys inside `config.json`
- Settings file: `~/.junie/config.json` · JSON · relocation `--config-location` / `JUNIE_CONFIG_LOCATION` (additive). Project scope: needs an ADR decision (`<project-root>/.junie/config.json`, honoured only in a trusted project)
- Per key: `/model` → off: cross-channel merge unverified (type string, scalar within config.json); `/effort` → off: same, values open; `/provider` → off: same (enum of seven documented); `/brave` → off: undefined semantics; `/byok`, `/hooks`, `/proxies` → off: credential-bearing or other category

### Zed
- Harness: no rows yet: the settings file path contradicts itself across Zed pages for macOS (`~/.config/zed/settings.json` versus `~/.zed/settings.json`) and Windows is unstated; per-tool lists are list-of-objects with unstated merge · re-check trigger: Zed docs state one settings file path per OS, then `/agent/tool_permissions/default` is the first row to add (enum, scalar; Linux alone is consistent)
- Settings file: Linux `~/.config/zed/settings.json` (custom `XDG_CONFIG_HOME` implied, not stated as relocation) · JSONC · Project scope: needs an ADR decision (`.zed/settings.json`)
- Per key: `/agent/tool_permissions/default` → off: path contradiction (otherwise enum `confirm`/`allow`/`deny`, scalar, global) · `/agent/tool_permissions/tools/*/always_*` → off: list-of-objects, merge unstated · `/agent/default_model`, `/agent/subagent_model` → off: object with open provider and effort values · marketplace → off: none

### OpenClaw
- Harness: no rows: no splice engine for the format (JSON5, confirmed by docs and by the live `config validate` run) · re-check trigger: grim gains a JSON5 splice engine, or OpenClaw documents plain-JSON `openclaw.json`
- Settings file: `~/.openclaw/openclaw.json` (Linux, macOS) · JSON5 · relocation: `OPENCLAW_STATE_DIR` / `OPENCLAW_HOME` per repo docs (not re-verified here)
- Per key: none

### Goose
- Harness: no rows: no splice engine for the format (YAML) · re-check trigger: grim gains a YAML splice engine
- Settings file: `~/.config/goose/config.yaml` · YAML · relocation: none documented on the page read
- Per key: none (candidates when the engine exists: `GOOSE_MODE`, `GOOSE_TEMPERATURE`, `GOOSE_TOOLSHIM`; env vars override the file)

## Issue drafts
none (findings for the orchestrator go in the report)

## Friction
- `npm install --ignore-scripts` leaves Amp and Junie without their native binaries; no workaround inside the sandbox rules.
- OpenClaw's launcher needs `node` on the restricted PATH; the node binary was copied into the scratch bin.
- The Junie page example carries a credential-shaped placeholder in a `byok` block; it is deliberately not reproduced here.

## Skill changes
none
