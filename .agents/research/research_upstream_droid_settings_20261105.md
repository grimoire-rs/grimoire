# Research: upstream refresh — droid settings (deep, #270)

## Metadata
**Date:** 2026-11-05 · **Domain:** vendor AI-config layouts · **Depth:** deep (settings row)
**Triggered by:** /hex-execute .agents/plans/plan_settings_follow_ups.md (R-A)
**Expires:** 2027-05-05

Pages fetched raw with `curl` as markdown (`<page>.md`), not through WebFetch.
No page shows a date. Vendor version: Droid CLI 1.3.0 (release notes, "CLI v1.3.0 ·
Desktop v1.3.0 (November 4, 2026)"), which the live check ran.

Sources: [settings](https://docs.factory.com/droid-cli/settings) (D1, redirected from
`/cli/configuration/settings`),
[hierarchy and merge semantics](https://docs.factory.com/enterprise/hierarchical-settings-and-org-control) (D2),
[sandbox](https://docs.factory.com/autonomy-and-safety/sandbox) (D3),
[plugins](https://docs.factory.com/harness/plugins) (D4),
[permission rules](https://docs.factory.com/autonomy-and-safety/permission-rules) (D5).

**Array merge (the scoping artifact's "array merge stated"), quoted:** D1, Disabled
skills: "User and project arrays are combined, so a skill disabled at either level
remains disabled." D2 states the general rule: "Arrays, union | Array fields
accumulate across levels. Org entries are always present; project, folder, and user
levels can add more without removing or weakening higher-level entries." D3: "Settings
merge across the hierarchy (org > project > user). `denyWrite`/`denyRead` use union
merge".

**Scalar merge, quoted (D2):** hard controls: "For scalar hard-control values
(strings, numbers, booleans), the first level that sets the value wins." Session
defaults: "The scalar keys inside `sessionDefaultSettings` … are exceptions to 'first
wins': the **most-local** value for each field wins".

## Claims

| # | Key (JSON pointer) | Value type and values | Merge (scalar / set / map; across which files) | Scope (global / project) | Category | Source URL | Verbatim quote | Page date / vendor version |
|---|---|---|---|---|---|---|---|---|
| 1 | (file) `~/.factory/settings.json` | JSON object | levels Org > Runtime > Folder > Project > User; `settings.local.json` on top at each level | global (and project) | — | D1, D2 | "macOS / Linux \| `~/.factory/settings.json`" · "Local overrides merge on top of the corresponding `settings.json` at the same level and follow the same hierarchy precedence." | none / 1.3.0 |
| 2 | `/sessionDefaultSettings/autonomyLevel` | enum `off`, `low`, `medium`, `high` | scalar; most-local wins, capped by `maxAutonomyLevel` | global | permission policy | D1, D2 | "Sets the default [Autonomy Level](/autonomy-and-safety/auto-run) for new sessions." · "Capped by `maxAutonomyLevel`." | none / 1.3.0 |
| 3 | `/sessionDefaultSettings/interactionMode` | enum `auto`, `spec`, `agi` | scalar; most-local wins | global | behaviour default | D1, D2 | D1: "`auto`, `spec`"; D2: "Droid interaction mode. One of `auto`, `spec`, `agi`." | none / 1.3.0 |
| 4 | `/sessionDefaultSettings/model` | string (model ID) | scalar; most-local wins | global | behaviour default | D2 | "Default model identifier." | none / 1.3.0 |
| 5 | `/sessionDefaultSettings/reasoningEffort` | enum `none`, `dynamic`, `off`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max` | scalar; most-local wins | global | behaviour default | D2, D1 | "One of `none`, `dynamic`, `off`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max`." · D1: "The available levels and the default are **set by each model**" | none / 1.3.0 |
| 6 | `/model`, `/reasoningEffort` (top level) | string; same enum as 5 | — | global | behaviour default | D1 | "`model` \| Any [available model ID](/models)" (D1 table, top level; D2 puts both inside `sessionDefaultSettings`) | none / 1.3.0 |
| 7 | `/outputStyle` | string: `default`, `concise`, or a custom style name | scalar | global | behaviour default | D1 | "`default`, `concise`, or a [custom style](/droid-cli/output-styles) selected in `/settings`" | none / 1.3.0 |
| 8 | `/diffMode` | enum `github`, `unified` | scalar | global | behaviour default | D1 | "Choose between split GitHub-style diffs and a single-column view." | none / 1.3.0 |
| 9 | `/soundFocusMode` | enum `always`, `focused`, `unfocused` | scalar | global | behaviour default | D1 | "When to play sound notifications." | none / 1.3.0 |
| 10 | `/completionSoundFocusMode`, `/awaitingInputSoundFocusMode` | enum `always`, `focused`, `unfocused` | scalar | global | behaviour default | D1 | "Override focus behavior for completion sounds." | none / 1.3.0 |
| 11 | `/toolResultDisplay` | enum `expanded`, `compact` | scalar | global | behaviour default | D1 | "How tool results are rendered in the transcript. Options: `expanded`, `compact`." | none / 1.3.0 |
| 12 | `/logoAnimation` | enum `once`, `always`, `off` | scalar | global | behaviour default | D1 | "Animate the droid logo on startup. Options: `once`, `always`, `off`." | none / 1.3.0 |
| 13 | `/subagentAutonomyLevel` | enum `inherit`, `off`, `low`, `medium`, `high` | scalar; hard control, first level wins | global | permission policy | D1, D2 | "`inherit`, `off`, `low`, `medium`, `high` \| `inherit` \| [Autonomy Level](/autonomy-and-safety/auto-run) applied to subagents spawned by the Task tool." (D1's enterprise table omits `off`) | none / 1.3.0 |
| 14 | `/includeCoAuthoredByDroid` | bool | scalar | global | behaviour default | D1 | "Automatically append the Droid co-author trailer to commits." | none / 1.3.0 |
| 15 | `/enableDroidShield` | bool | scalar | global | permission policy | D1 | "Enable secret scanning and git guardrails." | none / 1.3.0 |
| 16 | `/ideAutoConnect`, `/showThinkingInMainView`, `/streamAssistantResponses`, `/vimMode`, `/blockOnMcpLoad`, `/showTokenUsageIndicator`, `/overrideTerminalColors`, `/nerdFont` | bool | scalar | global | behaviour default | D1 | "`blockOnMcpLoad` \| `true`, `false` \| `false` \| Wait for MCP servers to finish loading before starting an agent turn." (one quote per key in D1's table) | none / 1.3.0 |
| 17 | `/disabledSkills` | string list (skill names) | set; user and project arrays combined | global | behaviour default | D1 | "User and project arrays are combined, so a skill disabled at either level remains disabled." | none / 1.3.0 |
| 18 | `/sandbox/enabled` | bool | scalar; hard control | global | permission policy | D3 | "Set `sandbox.enabled` to `true` in your settings to turn on the sandbox" | none / 1.3.0 |
| 19 | `/sandbox/mode` | enum `per-command`, `whole-process` | scalar; hard control | global | permission policy | D3 | "Isolation scope: \"per-command\" (default) or \"whole-process\"" | none / 1.3.0 |
| 20 | `/sandbox/filesystem/denyRead`, `/sandbox/filesystem/denyWrite` | string list (paths) | set; union across levels | global | permission policy | D3 | "`denyWrite`/`denyRead` use union merge: org denies cannot be removed downstream." | none / 1.3.0 |
| 21 | `/sandbox/filesystem/allowWrite`, `/sandbox/network/allowedDomains` | string list | set; D2 array union for hard controls (`sandbox` is one) | global | permission policy | D2, D3 | "Array fields accumulate across levels." · hard-control fields include "`sandbox`" | none / 1.3.0 |
| 22 | `/extraKnownMarketplaces/<name>` | object `{source: {source: github\|url\|local\|git-subdir, …}}` | map; D2 objects with locked keys, lower levels add new keys | global | pre-configured marketplaces | D2, D4 | "Additional plugin marketplaces. Map of marketplace name to a source object. Marketplaces listed here are automatically cloned and registered on CLI startup." · "user settings install as user scope" | none / 1.3.0 |
| 23 | `/hooksDisabled` | bool | scalar | global | — | D1 | "Globally disable all hooks execution." | none / 1.3.0 |
| 24 | `/completionSound`, `/awaitingInputSound` | `off`, `bell`, `fx-ok01`, `fx-ack01`, or a file path | scalar | global | behaviour default | D1 | "`off`, `bell`, `fx-ok01`, `fx-ack01`, or custom file path" | none / 1.3.0 |
| 25 | `/subagentSounds` | D1: `on`, `off` | scalar | global | behaviour default | D1 | "`subagentSounds` \| string \| `on`, `off` \| `off`" | none / 1.3.0 |
| 26 | `/theme`, `/todoDisplayMode` | string, values not listed | scalar | global | behaviour default | D1 | "Accepts a theme ID (see `/themes`)." · "Control how the todo list is shown in the terminal UI." | none / 1.3.0 |
| 27 | `/cloudSessionSync` | bool | scalar; hard control | global | — | D1 | "Mirror CLI sessions to Factory web." | none / 1.3.0 |
| 28 | `/permissionRules` | `{version: 1, rules: [{id, decision, match, tests}]}` | ID-keyed; "does not use ordinary array union" | global | permission policy | D1, D2, D5 | "`permissionRules.rules` does not use ordinary array union. … Higher-priority valid definitions claim repeated rule IDs; different IDs accumulate." | none / 1.3.0 |
| 29 | `/commandAllowlist`, `/commandDenylist`, `/commandBlocklist` | string list | union | global | permission policy | D1 | "**Deprecated.** Still respected for backward compatibility" | none / 1.3.0 |
| 30 | `/enabledPlugins/<id>` | bool map | additive | global | — | D2 | "By default, entries merge additively across org, project, and user settings." | none / 1.3.0 |
| 31 | `/customModels`, `/legacyModelOverrides`, `/statusLine`, number keys, enterprise keys (`maxAutonomyLevel`, `modelPolicy`, `mcpPolicy`, `strictKnownMarketplaces`, `strictEnabledPlugins`), mission keys | various | various | global | — | D1, D2 | "`customModels` \| Array of model configs \| `[]` \| Custom model configurations for BYOK." | none / 1.3.0 |

## Verifier sign-off
none required

## Live CLI check

npm `droid@1.3.0` (`npm install --ignore-scripts --prefix <scratch>`); the native
binary from the optional dependency `@factory/cli-linux-x64` ran directly. All runs:
`env -i HOME=<sb> TMPDIR=<sb> XDG_CONFIG_HOME=<sb> PATH=<sb>/bin:/usr/bin:/bin DO_NOT_TRACK=1 FACTORY_DROID_AUTO_UPDATE_ENABLED=false timeout 60 droid <verb> </dev/null`.

- `droid --version` — pass — `1.3.0`.
- `droid doctor --checks config --json` (no login, no model; reports `factoryDir: ~/.factory`) — **pass** — value runs accepted, bad-value controls rejected with a named diagnostic and exit 1:
  - Accepted every documented value of rows 2, 3 (incl. `agi`), 8-19, 20-21 and 22 (all four source shapes), a string for row 4, and `high` for row 5 (the other eight row-5 values ran only at the unvalidated top level); rejected each `bogus` / wrong-type control (`diffMode: bogus` → "Invalid enum value. Expected 'github' | 'unified'"; `vimMode: "yes"` → "Expected boolean, received string"; malformed JSON → "PropertyNameExpected").
  - Sandbox controls (`sandbox.mode: bogus`, string for `sandbox.enabled`, string for each path list) exit 1 through an uncaught "Invalid sandbox settings" error, not a JSON diagnostic: rejected, but see Friction.
  - Per-key skip (control passed, so doctor does not validate the key): top-level `/model` (number accepted), top-level `/reasoningEffort` (`bogus` accepted), `/outputStyle`, `/completionSound`, `/awaitingInputSound` (open values), `/hooksDisabled` (string and number accepted). `/sessionDefaultSettings/model` and `/reasoningEffort` are validated.
  - `/subagentSounds`: doctor rejects D1's documented value `on` (fail for the documented value).
- Format probe: a `//` comment is accepted; a trailing comma is rejected ("PropertyNameExpected").

## Routing

- Harness: rows · re-check trigger: a Droid release that changes the settings page tables, the D2 merge table, or the doctor's config validation.
- Settings file: `~/.factory/settings.json` (macOS/Linux), `%USERPROFILE%\.factory\settings.json` (Windows) · JSON (1.3.0 tolerates `//` comments, rejects trailing commas; the JSONC splice engine fits) · no relocating env var documented. Covered by the existing `droid-root` (`~/.factory`, `vendor_droid::droid_root`); no new `VendorRoot` row.
- Limits for every row: an org-managed value wins over a user value for hard controls, and a project or folder value wins for session defaults; `~/.factory/settings.local.json` merges on top of the user file, so a local override can mask a grim piece without changing the file grim reads. Droid creates the file with defaults on first run (D1).
- Project scope: `<project>/.factory/settings.json` and `settings.local.json` exist — project scope: needs an ADR decision.
- Per key:
  - `/sessionDefaultSettings/autonomyLevel` → row (enum `off`, `low`, `medium`, `high`, scalar, global)
  - `/sessionDefaultSettings/interactionMode` → row (enum `auto`, `spec`, `agi`, scalar, global). D1 lists two values, D2 three; the binary accepted all three in value runs.
  - `/sessionDefaultSettings/model` → row (string, scalar, global)
  - `/sessionDefaultSettings/reasoningEffort` → row, on-limited (enum, 9 values, scalar, global). Limit: each model accepts a subset.
  - `/outputStyle` → row (string, scalar, global), as Claude's `/outputStyle` row
  - `/diffMode`, `/soundFocusMode`, `/completionSoundFocusMode`, `/awaitingInputSoundFocusMode`, `/toolResultDisplay`, `/logoAnimation` → row (enum, scalar, global)
  - `/subagentAutonomyLevel` → row (enum `inherit`, `off`, `low`, `medium`, `high`, scalar, global)
  - `/includeCoAuthoredByDroid`, `/enableDroidShield`, `/ideAutoConnect`, `/showThinkingInMainView`, `/streamAssistantResponses`, `/vimMode`, `/blockOnMcpLoad`, `/showTokenUsageIndicator`, `/overrideTerminalColors`, `/nerdFont` → row (bool, scalar, global)
  - `/disabledSkills` → row (string list, set, global)
  - `/sandbox/enabled` → row (bool, scalar, global); `/sandbox/mode` → row (enum `per-command`, `whole-process`, scalar, global)
  - `/sandbox/filesystem/denyRead`, `/sandbox/filesystem/denyWrite` → row (string list, set, global)
  - `/sandbox/filesystem/allowWrite`, `/sandbox/network/allowedDomains` → row, on-limited (string list, set, global). Limit: merge comes from D2's general array-union rule, not a per-key statement.
  - `/extraKnownMarketplaces/<name>` → row (atomic object, keys `source`, map, global), the Claude row shape
  - `/model`, `/reasoningEffort` (top level) → off: D1 and D2 disagree on placement, and doctor 1.3.0 does not validate the top-level keys
  - `/hooksDisabled` → off: doctor 1.3.0 accepts any value for it, and turning hooks off globally would also silence hooks grim's hook kind installs
  - `/completionSound`, `/awaitingInputSound` → off: one documented value is any file path, and doctor accepts any string
  - `/subagentSounds` → off: D1 says `on`, `off`; 1.3.0 rejects `on` (values unresolved)
  - `/theme`, `/todoDisplayMode` → off: values not listed
  - `/cloudSessionSync` → off: data-sharing control, outside the three categories
  - `/permissionRules` → off: an ID-keyed, versioned rule container; no grim value type fits, and replacing it whole would drop the user's own rules
  - `/commandAllowlist`, `/commandDenylist`, `/commandBlocklist` → off: deprecated
  - `/enabledPlugins/<id>` → off: ADR Decision 3
  - `/customModels`, `/legacyModelOverrides`, `/statusLine`, number keys, enterprise and mission keys → off: BYOK credentials, legacy pins, a command, no integer type, org-managed only, not verified in this pass
- Variable expansion: none documented (2026-11-05, P-270 step 5; the reviewer fetched the settings pages).

## Issue drafts
none (findings for the orchestrator go in your report)

## Friction

- `/cli/configuration/settings.md` redirects to `/droid-cli/settings.md`; `/droid-cli/plugins.md` is a 404 (plugins live under `/harness/plugins`).
- `droid doctor --checks config` crashes with a stack trace on an invalid `sandbox` block instead of a JSON diagnostic (exit 1 either way).
- The CLI's `--help` still offers `exec` and an interactive session; only `doctor --checks config` and `--version` ran.

## Skill changes
none
