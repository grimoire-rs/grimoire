# Research: upstream refresh — antigravity settings (deep, #270)

## Metadata
**Date:** 2026-11-05 · **Domain:** vendor AI-config layouts · **Depth:** deep (settings row)
**Triggered by:** /hex-execute .agents/plans/plan_settings_follow_ups.md (R-A)
**Expires:** 2027-05-05

Pages were fetched raw with `curl` (markdown served at `<page>.md`), not through
WebFetch. The docs show no per-page date; `Last-Modified` is the site build time
(2026-11-05 01:17 GMT). Versions: the changelog's newest CLI entry is v1.4.12, its
newest App entry v2.29.0; the CLI's own update manifest served 1.5.0, which the
live check ran.

### PD-20: is grim's `antigravity` client the Antigravity CLI?

**Decision: yes, one product family.** grim's `antigravity` client (root
`~/.gemini/config`, `vendor_antigravity.rs`, verified against the 2.0 desktop
docs) already reaches the CLI. App and CLI share the harness and the
`~/.gemini/config/` customization root:

- Overview: "Every Antigravity surface runs on a shared, highly optimized agent harness".
- Customization discovery: "**Personal**: the `~/.gemini/config/` folder in your home directory, plus `~/.gemini/AGENTS.md` and `~/.gemini/GEMINI.md`. These customizations apply to every project, in both the Antigravity App and the Antigravity CLI."
- Live: `agy agents` (1.5.0) listed an agent placed at `~/.gemini/config/agents/`, and `agy mcp list` listed a server from `~/.gemini/config/mcp_config.json`.

The settings file is CLI-only and sits **outside** `antigravity-root`:
`~/.gemini/antigravity-cli/settings.json`. It needs one new append-only
`VendorRoot` row (proposed: row `antigravity-cli`, anchor
`antigravity-cli-root`, resolving to `<home>/.gemini/antigravity-cli`), added to
`VENDOR_ROOTS`, `SHIPPED_ANCHOR_TAGS` and `hermetic_vendor_roots` on the
`cursor-cli` precedent. Do not reuse `gemini-root`: it is another client's root
and follows `GEMINI_CLI_HOME`, which no Antigravity page documents. The App has
no documented on-disk settings file (its settings are a GUI), so these rows
configure the CLI only; the docs page must say so.

## Claims

Sources: [settings](https://antigravity.google/docs/settings) (S1),
[CLI reference](https://antigravity.google/docs/cli/reference) (S2),
[schema](https://antigravity.google/schemas/v1/settings.json) (S3, draft 2020-12,
title "Antigravity CLI settings", vendor-published),
[permissions](https://antigravity.google/docs/permissions) (S4),
[changelog](https://antigravity.google/docs/changelog) (S5).
Merge is within the one global file unless the row says otherwise; the CLI has
no project `settings.json`.

| # | Key (JSON pointer) | Value type and values | Merge (scalar / set / map; across which files) | Scope (global / project) | Category | Source URL | Verbatim quote | Page date / vendor version |
|---|---|---|---|---|---|---|---|---|
| 1 | (file) `~/.gemini/antigravity-cli/settings.json` | JSON object | sparse; rewritten by the CLI | global | — | S1, S2 | "This section lists every key you can set in `~/.gemini/antigravity-cli/settings.json`" · "**The file is normalized at startup.** When the CLI starts with a valid file, it rewrites the file in its own format, which also removes keys that are set to their default values." · "**Unknown keys are kept.**" | build 2026-11-05 / CLI 1.4.12 docs |
| 2 | `/agentMode` | enum `""`, `accept-edits`, `auto`, `plan` | scalar | global | behaviour default | S3, S2 | "Execution mode the CLI starts in: \"\" (default mode, which asks before file edits), \"accept-edits\" (approves workspace file edits automatically), \"auto" | schema v1 |
| 3 | `/toolPermission` | enum `request-review`, `proceed-in-sandbox`, `always-proceed`, `strict` | scalar | global | permission policy | S3, S2 | "When the agent asks before using a tool: \"request-review\" asks before write, bash, web, and MCP tools unless an allow rule covers them" | schema v1 |
| 4 | `/artifactReviewPolicy` | enum `asks-for-review`, `agent-decides`, `always-proceed` | scalar | global | behaviour default | S3 | "Whether the agent waits for your review when it asks for feedback on an artifact" | schema v1 |
| 5 | `/allowNonWorkspaceAccess` | bool | scalar | global | permission policy | S3 | "Lets the agent read and edit files outside your workspace folders, subject to toolPermission, agentMode, and your permission rules." | schema v1 |
| 6 | `/enableTerminalSandbox` | bool | scalar | global | permission policy | S3 | "Runs the agent's terminal commands in the terminal sandbox." | schema v1 |
| 7 | `/permissions/allow` | string list of `action(target)` | set; combined with the Project and Shared scopes | global | permission policy | S1, S3, S5 | "Each rule is an `action(target)` string … A deny rule takes precedence over an ask rule, and an ask rule takes precedence over an allow rule." · "The CLI combines them with the rules in the **Project** and **Shared** scopes." | schema v1; changelog v1.0.5 |
| 8 | `/permissions/ask` | as 7 | as 7 | global | permission policy | S1, S3 | as 7 | as 7 |
| 9 | `/permissions/deny` | as 7 | as 7 | global | permission policy | S1, S3 | "Actions the agent may never take. Deny rules take precedence over ask and allow rules." | schema v1 |
| 10 | `/model` | string (a model label) | scalar | global | behaviour default | S3 | "The model chosen with /model, saved by its label. If it's no longer available, the CLI uses the default model and shows a warning." | schema v1 |
| 11 | `/verbosity` | enum `high`, `medium`, `low` | scalar | global | behaviour default | S3 | "How much of the agent's work the conversation shows" | schema v1 |
| 12 | `/queuedMessages` | enum `queue`, `send-immediately` | scalar | global | behaviour default | S3 | "What happens to a message you send while the agent is working" | schema v1 |
| 13 | `/notifications` | bool | scalar | global | behaviour default | S3 | "Rings the terminal bell and shows a desktop notification when the agent needs your approval" | schema v1 |
| 14 | `/colorScheme` | enum `terminal`, `light`, `solarized light`, `colorblind-friendly light`, `dark`, `solarized dark`, `colorblind-friendly dark`, `tokyo night` | scalar | global | behaviour default | S3 | "Color theme. \"terminal\" uses your terminal's colors." | schema v1 |
| 15 | `/altScreenMode` | enum `default`, `always`, `never` | scalar | global | behaviour default | S3 | "How the TUI uses the terminal" | schema v1 |
| 16 | `/editorMode` | enum `default`, `vim` | scalar | global | behaviour default | S3 | "Editing model for the prompt: \"default\" or \"vim\" (modal editing)." | schema v1 |
| 17 | `/vimInsertFirst` | bool | scalar | global | behaviour default | S3 | "When editorMode is \"vim\", starts the prompt in Insert mode" | schema v1 |
| 18 | `/runningLightSpeed` | enum `fast`, `medium`, `slow`, `off` | scalar | global | behaviour default | S3 | "Speed of the progress indicator animation." | schema v1 |
| 19 | `/pickerGrouping` | enum `none`, `grouped` | scalar | global | behaviour default | S3 | "How the /resume picker lists conversations" | schema v1 |
| 20 | `/showTips` | bool | scalar | global | behaviour default | S3 | "Shows usage tips while the agent is generating a response." | schema v1 |
| 21 | `/showFeedbackSurvey` | bool | scalar | global | behaviour default | S3 | "Shows a brief feedback survey from time to time after a task finishes." | schema v1 |
| 22 | `/hideActiveItems` | bool | scalar | global | behaviour default | S3 | "Hides the list of active subagents and background tasks below the prompt box." | schema v1 |
| 23 | `/copyOnSelect` | bool | scalar | global | behaviour default | S3 | "In alt-screen mode, copies text to the clipboard when you release a mouse selection." | schema v1 |
| 24 | `/clearScrollbackOnResize` | bool | scalar | global | behaviour default | S3 | "In inline rendering, clears the terminal scrollback when the CLI redraws the conversation" | schema v1 |
| 25 | `/useTerminalCursor` | bool | scalar | global | behaviour default | S3 | "Uses your terminal's own cursor shape instead of a block cursor." | schema v1 |
| 26 | `/disableSlashCommands` | bool | scalar | global | behaviour default | S3 | "In headless mode (-p), sends prompts that start with / to the model as typed" | schema v1 |
| 27 | `/historySize` | integer, min -1 | scalar | global | behaviour default | S3 | "Number of prompt history entries kept on disk." | schema v1 |
| 28 | `/editor` | free string (a command) | scalar | global | — | S3 | "External editor for opening artifacts and files and for composing prompts (Ctrl+G)." | schema v1 |
| 29 | `/modelProvider` | enum `""`, `gemini` | scalar | global | — | S3 | "Set to \"gemini\" to send model requests directly to the Gemini API with the key in GEMINI_API_KEY." | schema v1 |
| 30 | `/customModelsConfig`, `/gcp` | object | — | global | — | S3 | (no description in the schema) | schema v1 |
| 31 | `/useG1Credits` | bool | scalar | global | — | S3 | "Uses your AI credits for model requests after your plan's quota runs out." | schema v1 |
| 32 | `/trustedWorkspaces` | string list | CLI-managed | global | — | S3 | "Folders you chose to trust when the CLI asked. The CLI adds entries for you." | schema v1 |
| 33 | `/statusLine`, `/title` | object (a script) | — | global | — | S3 | "Runs your own script to draw the status line below the prompt box." | schema v1 |
| 34 | `/sandboxProxy` | object (proxy, incl. a password) | — | global | — | S3 | (object; carries a credential field) | schema v1 |
| 35 | `/enableTelemetry`, `/hooks` | — | — | — | — | S3 | "Not read from settings.json." | schema v1 |
| 36 | (marketplaces) | none | — | — | pre-configured marketplaces | [marketplace](https://antigravity.google/docs/marketplace) | only the official `antigravity-plugins-official` marketplace is supported; no settings key | build 2026-11-05 |

## Verifier sign-off
none required

## Live CLI check

Binary: `agy` 1.5.0 from the CLI's own update manifest
(`antigravity-cli-auto-updater-…/manifests/linux_amd64.json`) → direct archive
`storage.googleapis.com/antigravity-public/antigravity-cli/1.5.0-…/linux-x64/cli_linux_x64.tar.gz`;
the manifest's sha512 matched. The installer script was read as text and never run.

- `env -i HOME=<sb> TMPDIR=<sb> XDG_CONFIG_HOME=<sb> PATH=<sb>/bin:/usr/bin:/bin AGY_CLI_DISABLE_AUTO_UPDATE=true timeout 60 agy --version </dev/null` — pass — prints `1.5.0`, exit 0.
- `agy --help` — pass — subcommands `acp`, `agent(s)`, `changelog`, `install`, `mcp`, `mic-serve`, `models`, `plugin(s)`, `remote-control`, `update`; no settings, config or doctor verb.
- Settings check with bad-value control `{"colorScheme": "bogus"}`: `agy mcp list`, `agy plugin list`, `agy agents` all exit 0 — skip — no credential-free verb reads `settings.json`; the control passed, so those verbs never read the file. The documented **Settings Error** screen is interactive and needs a session. `agy models` exits 1 asking to sign in — not run further.

## Routing

- Harness: rows · re-check trigger: a CLI release that changes `/schemas/v1/settings.json`, adds a project `settings.json`, or a `settings`/`doctor` verb.
- Settings file: `~/.gemini/antigravity-cli/settings.json` (no per-OS variant stated) · JSON · no relocating env var found (env vars seen: `AGY_CLI_DISABLE_AUTO_UPDATE`, `AGY_CLI_HIDE_ACCOUNT_INFO`, `AGY_LLM_GATEWAY_*`, `GEMINI_API_KEY`, `GOOGLE_GEMINI_BASE_URL`).
- Root: new `VendorRoot` row needed (see PD-20); `antigravity-root` (`~/.gemini/config`) does not cover the file.
- Limit for every row (Decision 11): the CLI rewrites the file at startup and drops any key equal to its default, so a piece whose value equals the default reads `missing` after the next CLI start until the next `grim install`. A wrong value opens a blocking Settings Error screen, so value checking must stay strict.
- Project scope: no project `settings.json`; project permission rules live under `~/.gemini/config/projects/` (S5 v1.0.12) — project scope: needs an ADR decision.
- Per key:
  - `/agentMode` → row (enum `accept-edits`, `auto`, `plan`, scalar, global). Leave out the `""` literal: it is the default and the CLI deletes it. `auto` is unavailable to business accounts, and the CLI falls back.
  - `/toolPermission` → row (enum, 4 values, scalar, global)
  - `/artifactReviewPolicy` → row (enum, 3 values, scalar, global)
  - `/allowNonWorkspaceAccess` → row (bool, scalar, global)
  - `/enableTerminalSandbox` → row (bool, scalar, global)
  - `/permissions/allow`, `/permissions/ask`, `/permissions/deny` → row, on-limited (string list, set, global). Limit: S5 v1.0.12 says project configurations "take precedence over global settings", which is older and less specific than S1's "combines"; precedence between the files is deny > ask > allow. The `unsandboxed(...)` action applies to business and enterprise accounts only.
  - `/model` → row (string, scalar, global)
  - `/verbosity`, `/queuedMessages`, `/colorScheme`, `/altScreenMode`, `/editorMode`, `/runningLightSpeed`, `/pickerGrouping` → row (enum, scalar, global)
  - `/notifications`, `/vimInsertFirst`, `/showTips`, `/showFeedbackSurvey`, `/hideActiveItems`, `/copyOnSelect`, `/clearScrollbackOnResize`, `/useTerminalCursor`, `/disableSlashCommands` → row (bool, scalar, global)
  - `/historySize` → off: integer, and grim has no integer value type
  - `/editor` → off: a free-form command the CLI runs
  - `/modelProvider`, `/customModelsConfig`, `/gcp` → off: provider and auth configuration, outside the three categories
  - `/useG1Credits` → off: billing, outside the three categories
  - `/trustedWorkspaces` → off: the CLI's own trust record; writing it pre-trusts folders (threat model T3/T4)
  - `/statusLine`, `/title` → off: run a script
  - `/sandboxProxy` → off: carries a credential
  - `/enableTelemetry`, `/hooks` → off: "Not read from settings.json"
  - marketplaces → no row: only the official marketplace, no settings key
- Variable expansion: none documented (2026-11-05, P-270 step 5; the reviewer fetched the settings pages).

## Issue drafts
none (findings for the orchestrator go in your report)

## Friction

- `docs/cli/settings.md`, `docs/cli.md`, `docs.md` and `docs/product/antigravity-cli.md` returned 404; the CLI settings live on `/docs/settings` (CLI tab) and `/docs/cli/reference`.
- No page carries a date; `Last-Modified` is the build time.
- 1.5.0 `--help` lists a `--settings` flag ("Settings for this session only, never saved") that the reference page (1.4.12) does not list yet.

## Skill changes
none
