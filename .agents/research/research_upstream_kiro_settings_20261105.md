# Research: upstream refresh — kiro settings (deep, #270)

## Metadata
**Date:** 2026-11-05 · **Domain:** vendor AI-config layouts · **Depth:** deep (settings row)
**Triggered by:** /hex-execute .agents/plans/plan_settings_follow_ups.md (R-A)
**Expires:** 2027-05-05

Pages fetched raw with `curl` as markdown (`<page>.md`), not through WebFetch.
Page dates as recorded by the scoping artifact (the `Last-Modified` header is the
site build time, 2026-11-04 21:53 GMT). Vendor version: `kiro-cli` 3.0.0
(`BUILD-INFO`: `BUILD_DATE=2026-11-02`), which the live check ran.

Sources: [settings reference](https://kiro.dev/docs/reference/settings) (K1,
2026-11-03), [configuration scopes](https://kiro.dev/docs/configuration) (K2,
2026-10-21), the binary's own `kiro-cli settings list --all --format json` (K3,
3.0.0; corroboration only, never the sole source for a value).

**Key shape:** Kiro keys are flat dotted names at the top level of the JSON object,
not nested objects (K1's own example: `{"chat.enableCustomSpinnerVerbs": true, …}`;
`kiro-cli settings list --format json` prints `{"chat.notificationMethod":"osc9",…}`).
The RFC 6901 pointer is therefore `/chat.defaultModel`, one segment.

### PD-21 (Kiro): global-only or per-key workspace override?

K2 (2026-10-21) lists "Settings (CLI) | `~/.kiro/settings/cli.json` | —" and
"Settings | ✓ | — | —". K1 (2026-11-03) says: "Pass `--workspace` to store a value
for the current project instead" and "When both scopes define a workspace-overridable
setting, the workspace value takes precedence in that project's terminal UI sessions.
… Some settings are global-only — including `chat.agentEngine`, `chat.uiMode`, and
`chat.terminalTitle` — and cannot be overridden pe[r workspace]".

**Decision: the newer page (K1) wins; the credential-free check agrees.** With
`{"chat.defaultAgent": "global-agent", "chat.terminalTitle": false}` in the global file
and `{"chat.defaultAgent": "ws-agent", "chat.terminalTitle": true}` in
`<proj>/.kiro/settings/cli.json`, `kiro-cli settings chat.defaultAgent` run from
`<proj>` printed `ws-agent (workspace)`, and `kiro-cli settings chat.terminalTitle`
printed `false (global)`. A project file exists and overrides per key, except for
global-only keys. Global rows are unaffected; project scope needs an ADR decision.

## Claims

| # | Key (JSON pointer) | Value type and values | Merge (scalar / set / map; across which files) | Scope (global / project) | Category | Source URL | Verbatim quote | Page date / vendor version |
|---|---|---|---|---|---|---|---|---|
| 1 | (file) `~/.kiro/settings/cli.json` | JSON object, flat dotted keys | per key: workspace value wins over global, except global-only keys | global (+ workspace) | — | K1, K2 | "both surfaces use the CLI settings stored in `~/.kiro/settings/cli.json`" · "When both scopes define a workspace-overridable setting, the workspace value takes precedence" | 2026-11-03 / 3.0.0 |
| 2 | `/chat.defaultModel` | string (model ID) | scalar | global | behaviour default | K1 | "Default AI model for conversations" · "If that model is unavailable, Kiro uses the service-provided default and shows a notice. The setting is not rewritten." | 2026-11-03 / 3.0.0 |
| 3 | `/chat.defaultAgent` | string (agent name) | scalar | global | behaviour default | K1 | "Default agent configuration" | 2026-11-03 / 3.0.0 |
| 4 | `/chat.agentEngine` | enum `v3`, `v2`, `v1` | scalar; global-only | global | behaviour default | K1 | "Agent harness for `kiro-cli` and `kiro-cli chat`: `v3`, `v2`, or `v1`. … `v1` selected the retired Classic interface and now opens the Classic upgrade prompt." | 2026-11-03 / 3.0.0 |
| 5 | `/acp.agentEngine` | enum `v3`, `v2`, `v1` | scalar | global | behaviour default | K1 | "Agent harness for `kiro-cli acp`: `v3`, `v2`, or `v1` (Kiro CLI 2.36 or later)." | 2026-11-03 / 3.0.0 |
| 6 | `/chat.notificationMethod` | enum `auto`, `bel`, `osc9` | scalar | global | behaviour default | K1, K3 | "Notification method: `auto`, `bel`, or `osc9`" | 2026-11-03 / 3.0.0 |
| 7 | `/chat.historyMode` | enum `session`, `global` | scalar | global | behaviour default | K1 | "Prompt history scope: `session` (per-session, default) or `global` (shared across sessions)." | 2026-11-03 / 3.0.0 |
| 8 | `/chat.screenReader` | enum `auto`, `on`, `off` | scalar; global-only | global | behaviour default | K1 | "Full terminal UI screen reader support: `auto`, `on`, or `off`. … Global only." | 2026-11-03 / 3.0.0 |
| 9 | `/chat.workflowLaunch` | enum `when-asked`, `large-tasks`, `delegate-all` | scalar | global | behaviour default | K1 | "Choose when V3 starts Workflows: `when-asked` (default), `large-tasks`, or `delegate-all`" | 2026-11-03 / 3.0.0 |
| 10 | `/chat.sessionDashboard.scope` | enum `current`, `all` | scalar | global | behaviour default | K1 | "V3 `/sessions` directory scope: `current` lists sessions from the current directory, `all` lists every workspace" | 2026-11-03 / 3.0.0 |
| 11 | `/chat.outputStyle` | string (style name; example `concise`) | scalar | global | behaviour default | K1 | "V3 response style selected under `/settings display`; a workspace value takes precedence over the global value" | 2026-11-03 / 3.0.0 |
| 12 | `/chat.disableGranularTrust` | bool | scalar | global | permission policy | K1 | "Disable tiered trust options for tool approvals (terminal UI only)" | 2026-11-03 / 3.0.0 |
| 13 | `/chat.enableNotifications`, `/chat.greeting.enabled`, `/chat.disableWrap`, `/chat.disableAutoCompaction`, `/chat.enablePromptHints`, `/chat.autoExpandToolOutput`, `/chat.sessionDashboard.indexResponses` | bool | scalar | global | behaviour default | K1 | "`chat.disableAutoCompaction` \| boolean \| Disable automatic conversation summarization" (one row per key) | 2026-11-03 / 3.0.0 |
| 14 | `/chat.allowAnimations`, `/chat.allowAsciiArt`, `/chat.allowIcons`, `/chat.preserveScrollback`, `/chat.terminalTitle` (global-only), `/chat.showThinking` | bool | scalar | global | behaviour default | K1 | "`chat.terminalTitle` \| boolean \| `false` \| Set the terminal window title … Global only" | 2026-11-03 / 3.0.0 |
| 15 | `/chat.enableKnowledge`, `/chat.enableCodeIntelligence`, `/chat.enableThinking`, `/chat.enableWorkflows`, `/toolSearch.enabled` | bool | scalar | global | behaviour default | K1 | "V3 enables knowledge and code intelligence by default. Set `chat.enableKnowledge` or `chat.enableCodeIntelligence` to `false` to turn either tool off." | 2026-11-03 / 3.0.0 |
| 16 | `/chat.enableAutoAgentUpgrade` | bool | scalar; global-only | global | behaviour default | K1 | "Upgrade V2-format agents in the scanned agent directories to the V3 format when a V3 session starts. … Global only" | 2026-11-03 / 3.0.0 |
| 17 | `/app.disableAutoupdates`, `/app.warnOnElevatedPrivileges` (global-only) | bool | scalar | global | behaviour default | K1 | "This setting is on by default and is global; a workspace's `.kiro/settings/cli.json` cannot override it" | 2026-11-03 / 3.0.0 |
| 18 | `/knowledge.defaultIncludePatterns`, `/knowledge.defaultExcludePatterns` | string list (file patterns) | set within the file; a workspace value replaces the key | global | behaviour default | K1 | "Default file patterns to include" · "Array values: Use JSON format with single quotes" | 2026-11-03 / 3.0.0 |
| 19 | `/telemetry.enabled`, `/telemetryClientId` | bool; string | scalar | global | — | K1 | "Enable/disable telemetry collection" | 2026-11-03 / 3.0.0 |
| 20 | classic-only keys: `/chat.diffTool`, `/chat.editMode`, `/chat.disableMarkdownRendering`, `/chat.enableHistoryHints`, `/chat.enableContextUsageIndicator`, `/chat.enableTangentMode`, `/introspect.tangentMode`, `/chat.enableCheckpoint`, `/chat.enableDelegate`, `/chat.enableTodoList`, classic key bindings | bool / string / char | scalar | global | — | K1 | "In Kiro CLI 3.0.0 or later, Classic no longer runs for interactive chat, so these settings have no effect in a terminal UI session." | 2026-11-03 / 3.0.0 |
| 21 | `/chat.uiMode`, `/chat.ui` | string | scalar | global | — | K1 | "Legacy message-rendering mode … has no effect in Kiro CLI 3.0.0 or later" | 2026-11-03 / 3.0.0 |
| 22 | number keys: `/compaction.*`, `/knowledge.maxFiles`, `/knowledge.chunkSize`, `/knowledge.chunkOverlap`, `/api.*` timeouts, `/mcp.initTimeout`, `/mcp.noInteractiveTimeout`, `/toolSearch.minPct`, `/toolSearch.minTokens` | number | scalar | global | — | K1 | "`api.timeout` \| number \| Overall timeout in seconds for a streaming response (default `3600`)." | 2026-11-03 / 3.0.0 |
| 23 | `/knowledge.indexType` | string, values not listed | scalar | global | — | K1 | "Type of knowledge index" | 2026-11-03 / 3.0.0 |
| 24 | `/chat.modelDefaults`, `/chat.spinnerVerbs`, `/chat.enableCustomSpinnerVerbs` | object; object; bool | — | global | — | K1 | "Per-model default settings (e.g., effort level) that apply across new sessions" · "A verb pool with `append` or `replace` mode" | 2026-11-03 / 3.0.0 |
| 25 | `/chat.keybindings.cancelStream`, `/chat.keybindings.closeMenu`, `/chat.keybindings.quit` | string | scalar | global | — | K1 | "Invalid values fall back to the built-in default." | 2026-11-03 / 3.0.0 |
| 26 | `/mcp.loadedBefore` | bool | scalar | global | — | K1 | "Track previously loaded MCP servers" | 2026-11-03 / 3.0.0 |
| 27 | keys listed by K3 only (`chat.defaultInterruptBehavior`, `memory.mode`, `chat.disableTrustAllConfirmation`, `trust.classifier.*`, `api.*.service`, `install.releaseUrl`, …) | various | — | global | — | K3 | "Skip the trust-all-tools confirmation gate on startup (boolean)" (K3, `chat.disableTrustAllConfirmation`) | 3.0.0 |
| 28 | permissions file `~/.kiro/settings/permissions.yaml` | YAML | deny-overrides | global | permission policy | K2 | "Permissions use a deny-overrides algorithm — a `deny` rule in any scope blocks the action regardless of `allow` rules elsewhere." | 2026-10-21 |
| 29 | sandbox file `~/.kiro/settings/sandbox.json` | JSON, a separate file | "An active global policy is the floor" | global | permission policy | K2 | "An active global policy is the floor; a project can turn it on or tighten it, but cannot weaken it" | 2026-10-21 |

## Verifier sign-off
none required

## Live CLI check

Binary: `kiro-cli` 3.0.0 from the direct archive
`https://desktop-release.q.us-east-1.amazonaws.com/latest/kirocli-x86_64-linux.zip`;
the published `kirocli-x86_64-linux.zip.sha256` matched
(`83dd96e0…d8be8`). The zip was extracted with path checks; its `install.sh` never ran.
All runs: `env -i HOME=<sb> TMPDIR=<sb> XDG_CONFIG_HOME=<sb> PATH=<sb>/bin:/usr/bin:/bin DO_NOT_TRACK=1 timeout 60 kiro-cli <verb> </dev/null`.

- `kiro-cli --version` — pass — `kiro-cli 3.0.0`.
- `kiro-cli settings list --all --format json` — pass — lists every key with a one-line description (K3).
- Settings check, value run `{"chat.notificationMethod": "osc9", "chat.greeting.enabled": false}` → `settings list --format json` prints both values; `settings chat.notificationMethod` prints `osc9 (global)`. Controls: malformed JSON → exit 1, "failed to parse … EOF while parsing a value"; `chat.notificationMethod: "bogus"` and `chat.greeting.enabled: "yes"` → exit 0, printed back unchanged — **skip** for value validation: the verb reads the file but checks no value. The set verb (`kiro-cli settings chat.notificationMethod bogus`, scratch file only) also stores bad values unchecked.
- Structural facts confirmed (ran and passed): the file path, the flat dotted key shape, `KIRO_HOME=<dir>` relocates the file to `<dir>/settings/cli.json` (printed `khome-agent (global)`), and the PD-21 workspace override above.
- Format probe: a `//` comment → "key must be a string at line 2 column 3"; a trailing comma → "trailing comma at line 3 column 1". Strict JSON.

## Routing

- Harness: rows · re-check trigger: a Kiro CLI release that adds value validation to `kiro-cli settings`, changes K1's tables, or reconciles K2's scope table.
- Settings file: `~/.kiro/settings/cli.json` (no per-OS variant stated) · strict JSON (no comments, no trailing commas; the CLI writes it pretty-printed, mode 0600, beside a `cli.json.lock`) · `KIRO_HOME` relocates it to `$KIRO_HOME/settings/cli.json` (K1, verified live). Covered by the existing `kiro-root` (`$KIRO_HOME` else `~/.kiro`, `vendor_kiro::kiro_root`); no new `VendorRoot` row.
- Limits for every row: the CLI does not validate values, so a wrong value is stored and then ignored or defaulted at run time (K1: "Invalid values fall back to the built-in default" for key bindings); grim's own value check is the only check. A workspace `.kiro/settings/cli.json` value overrides a global piece for workspace-overridable keys.
- Project scope: workspace file confirmed (PD-21) — project scope: needs an ADR decision, including the global-only key list.
- Per key:
  - `/chat.defaultModel`, `/chat.defaultAgent`, `/chat.outputStyle` → row (string, scalar, global)
  - `/chat.agentEngine`, `/acp.agentEngine` → row (enum `v3`, `v2`, scalar, global). Leave out `v1`: it is retired and opens an upgrade prompt. K3's descriptions still call `v2` the default; K1 says unset starts V3 in 3.0.0 — defaults only, the value set agrees.
  - `/chat.notificationMethod`, `/chat.historyMode`, `/chat.screenReader`, `/chat.workflowLaunch`, `/chat.sessionDashboard.scope` → row (enum, scalar, global)
  - `/chat.disableGranularTrust` → row (bool, scalar, global)
  - Bools in rows 13-17 → row (bool, scalar, global). `/chat.showThinking`: K1 says default `true`, K3 says `false`; defaults only.
  - `/knowledge.defaultIncludePatterns`, `/knowledge.defaultExcludePatterns` → row, on-limited (string list, set, global). Limit: a workspace value replaces the whole key.
  - `/telemetry.enabled`, `/telemetryClientId` → off: data-sharing control and an identifier, outside the three categories
  - classic-only keys, `/chat.uiMode`, `/chat.ui` → off: no effect in Kiro CLI 3.0.0
  - number keys → off: no integer value type
  - `/knowledge.indexType` → off: values not listed
  - `/chat.modelDefaults`, `/chat.spinnerVerbs`, `/chat.enableCustomSpinnerVerbs` → off: object shapes documented elsewhere or cosmetic; spinner verbs may carry URLs
  - `/chat.keybindings.*` → off: key bindings, outside the three categories
  - `/mcp.loadedBefore` → off: the CLI's own state
  - K3-only keys → off: not in the docs (primary-source rule); `chat.disableTrustAllConfirmation` would also weaken a trust gate
  - permissions → no rows: `permissions.yaml` is YAML, no splice engine for the format · re-check trigger: a JSON permissions file or a YAML splice engine in grim
  - sandbox → no rows here: a separate file (`settings/sandbox.json`), not `cli.json`; a second settings file per client would need an ADR decision
- Variable expansion: none documented (2026-11-05, P-270 step 5; the reviewer fetched the settings pages).

## Issue drafts
none (findings for the orchestrator go in your report)

## Friction

- K2 contradicts K1 on settings scope; K1 and the binary win (PD-21).
- `kiro-cli settings` offers no validation verb, so the value check could not be exercised; only structure was confirmed.
- `https://kiro.dev/changelog/cli.md` returned 404.

## Skill changes
none
