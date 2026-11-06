# Research: upstream refresh — gemini settings (deep, #270)

## Metadata
**Date:** 2026-11-05 · **Domain:** vendor AI-config layouts · **Depth:** deep (settings row)
**Triggered by:** /hex-execute .agents/plans/plan_settings_follow_ups.md (R-A)
**Expires:** 2027-05-05

All sources fetched raw with `curl` from the vendor repo's `main` branch on
2026-11-05, not through WebFetch. Vendor version: `@google/gemini-cli` 0.67.0 (npm
`latest`, modified 2026-11-04), which the live check ran.

Sources: [configuration reference](https://github.com/google-gemini/gemini-cli/blob/main/docs/reference/configuration.md) (G1),
[settingsSchema.ts](https://github.com/google-gemini/gemini-cli/blob/main/packages/cli/src/config/settingsSchema.ts) (G2, vendor source: merge strategies),
[settings.schema.json](https://github.com/google-gemini/gemini-cli/blob/main/schemas/settings.schema.json) (G3, generated from G2, linked by G1),
[transition post](https://developers.googleblog.com/an-important-update-transitioning-gemini-cli-to-antigravity-cli/) (G4, May 19, 2026).

### Is Gemini CLI still worth rows, and does grim's `gemini` client map to a live product?

**Decision: yes to both, with a narrower audience.** G4: "On June 18, 2026, Gemini
CLI and Gemini Code Assist IDE extensions will stop serving requests for Google AI Pro
and Ultra, as well as those using it free of charge using Gemini Code Assist for
individuals." But: "If your organization uses Gemini CLI or our IDE extensions via a
Gemini Code Assist Standard or Enterprise license … your access remains unchanged.
We'll continue to support Gemini CLI … Gemini CLI will remain accessible via paid
Gemini and Gemini Enterprise Agent Platform API keys." The repo is not archived (last
push 2026-11-05) and npm shipped 0.67.0 on 2026-11-04. grim's `gemini` client
(`~/.gemini`, `vendor_gemini.rs`) is that live CLI; consumer users moved to the
Antigravity CLI, which is grim's `antigravity` client. Ship Gemini rows after the
Antigravity rows.

**Merge statements.** G1: "Project settings override user settings and system
defaults." and "System settings act as overrides, taking precedence over all other
settings files." G2 states per-key array merge: `enum MergeStrategy { // Replace the old value
with the new value. This is the default. REPLACE = 'replace', // Concatenate arrays.
CONCAT = 'concat', // Merge arrays, ensuring unique values. UNION = 'union', … }`.
`tools.exclude`, `skills.disabled`, `extensions.disabled` and
`context.fileFiltering.customIgnoreFilePaths` carry `mergeStrategy: MergeStrategy.UNION`;
`tools.allowed`, `tools.core`, `tools.confirmationRequired` and
`tools.sandboxAllowedPaths` carry none, so they REPLACE across files.

## Claims

| # | Key (JSON pointer) | Value type and values | Merge (scalar / set / map; across which files) | Scope (global / project) | Category | Source URL | Verbatim quote | Page date / vendor version |
|---|---|---|---|---|---|---|---|---|
| 1 | (file) `~/.gemini/settings.json` | JSON object, nested categories | system defaults < user < project < system overrides | global (+ project) | — | G1 | "**Location:** `~/.gemini/settings.json` (where `~` is your home directory)." · "`GEMINI_CLI_HOME` … Specifies the root directory for Gemini CLI's user-level configuration and storage. … The CLI will create a `.gemini` folder inside this directory." | main 2026-11-05 / 0.67.0 |
| 2 | `/general/defaultApprovalMode` | enum `default`, `auto_edit`, `plan` | scalar | global | permission policy | G1 | "'default' prompts for approval, 'auto_edit' auto-approves edit tools, and 'plan' is read-only mode. YOLO mode (auto-approve all actions) can only be enabled via command line" | main / 0.67.0 |
| 3 | `/general/notificationMethod` | enum `auto`, `osc9`, `osc777`, `bell` | scalar | global | behaviour default | G1 | "How to send terminal notifications." | main / 0.67.0 |
| 4 | `/general/preferredEditor` | enum `vscode`, `vscodium`, `windsurf`, `cursor`, `zed`, `antigravity`, `sublimetext`, `lapce`, `nova`, `bbedit`, `vim`, `neovim`, `emacs`, `hx`, `emacsclient`, `micro` | scalar | global | behaviour default | G1 | "Must be one of the built-in supported identifiers." | main / 0.67.0 |
| 5 | `/general/vimMode`, `/general/enableNotifications`, `/general/checkpointing/enabled`, `/general/plan/enabled`, `/general/enableAutoUpdate`, `/general/enableAutoUpdateNotification` | bool | scalar | global | behaviour default | G1 | "Enable Plan Mode for read-only safety during planning." (one entry per key) | main / 0.67.0 |
| 6 | `/model/name` | string | scalar | global | behaviour default | G1 | "The Gemini model to use for conversations." | main / 0.67.0 |
| 7 | `/output/format` | enum `text`, `json` | scalar | global | behaviour default | G1 | "The format of the CLI output. Can be `text` or `json`." | main / 0.67.0 |
| 8 | `/ui/inlineThinkingMode` | enum `off`, `full` | scalar | global | behaviour default | G1 | "Display model thinking inline: off or full." | main / 0.67.0 |
| 9 | `/ui/loadingPhrases` | enum `tips`, `witty`, `all`, `off` | scalar | global | behaviour default | G1 | "What to show while the model is working: tips, witty comments, all, or off." | main / 0.67.0 |
| 10 | `/ui/errorVerbosity` | enum `low`, `full` | scalar | global | behaviour default | G1 | "Controls whether recoverable errors are hidden (low) or fully shown (full)." | main / 0.67.0 |
| 11 | `/ui/hideTips`, `/ui/hideBanner`, `/ui/showLineNumbers`, `/ui/compactToolOutput` | bool | scalar | global | behaviour default | G1 | "Hide the application banner" (one entry per key) | main / 0.67.0 |
| 12 | `/tools/shell/backgroundCompletionBehavior` | enum `silent`, `inject`, `notify` | scalar | global | behaviour default | G1 | "Controls what happens when a background shell command finishes." | main / 0.67.0 |
| 13 | `/tools/allowed` | string list (tool names, e.g. `run_shell_command(git)`) | set in the file; REPLACE across files | global | permission policy | G1, G2 | "Tool names that bypass the confirmation dialog." · G2: no `mergeStrategy` on `allowed` | main / 0.67.0 |
| 14 | `/tools/confirmationRequired` | string list | set in the file; REPLACE across files | global | permission policy | G1, G2 | "Tool names that always require user confirmation. Takes precedence over allowed tools and core tool allowlists." | main / 0.67.0 |
| 15 | `/tools/exclude` | string list | set; UNION across files | global | permission policy | G1, G2 | "Tool names to exclude from discovery." · `mergeStrategy: MergeStrategy.UNION` | main / 0.67.0 |
| 16 | `/tools/core` | string list | REPLACE across files | global | permission policy | G1 | "Restrict the set of built-in tools with an allowlist." | main / 0.67.0 |
| 17 | `/tools/sandbox` | bool, profile path or command string | scalar | global | permission policy | G1 | "Set to a boolean to enable or disable the sandbox, provide a string path to a sandbox profile, or specify an explicit sandbox command" | main / 0.67.0 |
| 18 | `/tools/sandboxNetworkAccess` | bool | scalar | global | permission policy | G1 | "Whether the sandbox is allowed to access the network." | main / 0.67.0 |
| 19 | `/tools/sandboxAllowedPaths` | string list (paths) | set in the file; REPLACE across files | global | permission policy | G1, G2 | "List of additional paths that the sandbox is allowed to access." | main / 0.67.0 |
| 20 | `/security/toolSandboxing`, `/security/disableYoloMode`, `/security/disableAlwaysAllow`, `/security/enablePermanentToolApproval`, `/security/autoAddToPolicyByDefault`, `/security/blockGitExtensions`, `/security/folderTrust/enabled` | bool | scalar | global | permission policy | G1 | "Disable YOLO mode, even if enabled by a flag." (one entry per key) | main / 0.67.0 |
| 21 | `/security/allowedExtensions` | string list (regex) | REPLACE | global | permission policy | G1 | "If nonempty, only extensions that match the patterns in this list are allowed." | main / 0.67.0 |
| 22 | `/skills/disabled` | string list | set; UNION across files | global | behaviour default | G2 | "List of disabled skills." · `mergeStrategy: MergeStrategy.UNION` | main / 0.67.0 |
| 23 | `/context/fileFiltering/respectGitIgnore`, `/context/fileFiltering/respectGeminiIgnore` | bool | scalar | global | behaviour default | G1 | "Respect .gitignore files when searching." | main / 0.67.0 |
| 24 | `/ui/theme` | string, values in another guide | scalar | global | behaviour default | G1 | "See the CLI themes guide for available options." | main / 0.67.0 |
| 25 | `/privacy/usageStatisticsEnabled` | bool | scalar | global | — | G1 | (privacy toggle) | main / 0.67.0 |
| 26 | `/security/auth/*`, `/security/environmentVariableRedaction/*`, `/mcpServers`, `/hooks/*`, `/hooksConfig/*`, `/experimental/*`, `/admin/*`, `/context/includeDirectories`, number keys | various | various | global | — | G1, G2 | "`mcpServers` … `mergeStrategy: MergeStrategy.SHALLOW_MERGE`" (G2) | main / 0.67.0 |

## Verifier sign-off
none required

## Live CLI check

npm `@google/gemini-cli@0.67.0` (`npm install --ignore-scripts --prefix <scratch>`).
All runs: `env -i HOME=<sb> TMPDIR=<sb> XDG_CONFIG_HOME=<sb> PATH=<sb>/bin:/usr/bin:/bin DO_NOT_TRACK=1 GEMINI_CLI_NO_RELAUNCH=true timeout 60 gemini --version </dev/null`.

- `gemini --version` reads and validates `~/.gemini/settings.json` before printing — **pass**. Keys exercised (each with a value run and a control): `general.defaultApprovalMode` (all 3), `general.vimMode`, `general.enableNotifications`, `general.notificationMethod` (all 4), `general.preferredEditor` (`vscode`, `neovim`), `general.checkpointing.enabled`, `general.plan.enabled`, `general.enableAutoUpdate`, `ui.inlineThinkingMode`, `ui.loadingPhrases`, `ui.errorVerbosity` (all values), `ui.hideTips`, `ui.theme`, `model.name`, `tools.shell.backgroundCompletionBehavior` (all 3), `tools.allowed`, `tools.exclude`, `tools.confirmationRequired`, `tools.sandboxNetworkAccess`, the seven `security.*` bools of row 20, `context.fileFiltering.respectGitIgnore`, `skills.disabled`, `output.format` (both). Other keys in grouped rows were not exercised. Every value run was accepted silently; every bad-value control was rejected with "Invalid configuration in …/settings.json: Error in: <key> …" (for example `general.defaultApprovalMode: "yolo"` → "Invalid enum value. Expected 'default' | 'auto_edit' | 'plan', received 'yolo'"; a string for `tools.allowed` → "Expected array, received string"). Bad values still exit 0 (diagnostic only); malformed JSON exits 52 ("Please fix the configuration file(s) and try again.").
- Format probe: a file with a `//` comment and a trailing comma failed only at the trailing comma, so comments are stripped and trailing commas rejected.

## Routing

- Harness: rows · re-check trigger: Google ending Gemini CLI for enterprise and API-key users, or a G2 change to a row's type or `mergeStrategy`.
- Settings file: `~/.gemini/settings.json` on every OS; `GEMINI_CLI_HOME` relocates it to `$GEMINI_CLI_HOME/.gemini/settings.json` · JSON (comments tolerated, trailing commas rejected) · covered by the existing `gemini-root` (`vendor_gemini::gemini_root`), no new `VendorRoot` row. grim already writes `mcpServers` into this same file, so settings pieces share it with MCP registration.
- Limits for every row: a project `.gemini/settings.json` and the system settings file override user values (G1).
- Project scope: `.gemini/settings.json` exists — project scope: needs an ADR decision.
- Per key:
  - `/general/defaultApprovalMode` → row (enum `default`, `auto_edit`, `plan`, scalar, global)
  - `/general/notificationMethod`, `/general/preferredEditor`, `/output/format`, `/ui/inlineThinkingMode`, `/ui/loadingPhrases`, `/ui/errorVerbosity`, `/tools/shell/backgroundCompletionBehavior` → row (enum, scalar, global)
  - `/model/name` → row (string, scalar, global)
  - Bools in rows 5, 11, 18, 20, 23 → row (bool, scalar, global)
  - `/tools/exclude`, `/skills/disabled` → row (string list, set, global)
  - `/tools/allowed`, `/tools/confirmationRequired`, `/tools/sandboxAllowedPaths` → row, on-limited (string list, set, global). Limit: a project file's list replaces the global one (REPLACE).
  - `/tools/core` → off: a non-empty list is an allowlist, so one grim element would switch off every other built-in tool
  - `/security/allowedExtensions` → off: same allowlist hazard
  - `/tools/sandbox` → off: union of bool, path and command; a command runs
  - `/ui/theme` → off: values listed on another page, not resolved here
  - `/privacy/usageStatisticsEnabled` → off: data-sharing control, outside the three categories
  - auth, redaction lists, `mcpServers`, hooks, experimental, admin, include directories, number keys → off: credentials or auth, not verified in this pass, other kinds (MCP, hook), experimental, admin-managed, machine paths, no integer type
  - marketplaces → no row: no marketplace key in G1

## Issue drafts
none (findings for the orchestrator go in your report)

## Friction

- The configuration page states precedence between files but not array merge; the merge class came from the vendor source (G2).
- A bad value does not change the exit code, only prints a diagnostic; a check that reads only the exit code would wrongly pass.

## Skill changes
none

## Note: variable expansion (2026-11-05, P-270 step 2c)

Gemini resolves `$NAME`, `${NAME}` and `${NAME:-default}` in every string
value of `settings.json`, keys excluded. Sources: the
[configuration reference](https://github.com/google-gemini/gemini-cli/blob/main/docs/reference/configuration.md)
and [`packages/cli/src/utils/envVarResolver.ts`](https://github.com/google-gemini/gemini-cli/blob/main/packages/cli/src/utils/envVarResolver.ts).
A delivered value could therefore read the user's environment at load time.
grim refuses a Gemini string value holding `$` before a letter, digit, `_` or
`{` at build with `holds an expansion token` (exit 65), as it already does for
OpenCode's `{env:` and `{file:` (PD-37). The Claims and Routing sections above
are unchanged.

## Note: folder trust (2026-11-05, P-270 step 5)

`/security/folderTrust/enabled` → off: switches off the harness's folder trust
gate (threat model T3). With `false`, an untrusted repository's
`.gemini/settings.json`, `.env` and MCP servers load. The Antigravity and Kiro
artifacts give the same reason for keeping `/trustedWorkspaces` and
`chat.disableTrustAllConfirmation` off. The other six `/security/*` bools of
row 20 stay rows: they set sandbox, approval and extension-source policy, not a
trust gate. The Routing line "Bools in rows 5, 11, 18, 20, 23 → row" excludes
this key; Gemini ships 33 rows.
