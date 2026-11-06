# Research: upstream refresh — kilo settings (deep, #270)

## Metadata
**Date:** 2026-11-05 · **Domain:** vendor AI-config layouts · **Depth:** deep (settings row)
**Triggered by:** /hex-execute .agents/plans/plan_settings_follow_ups.md (R-A)
**Expires:** 2027-05-05

Fetched raw with `curl`, not through WebFetch. Kilo's docs site serves its full
corpus at `https://kilo.ai/docs/llms.txt` (1.5 MB); quotes below come from that file,
each attributed to the page its heading names. `kilo.ai/docs/code-with-ai/platforms/cli.md`
returned 404. No page shows a date. Vendor version: `@kilocode/cli` 7.8.18 (npm latest),
which the live check ran.

Sources: Kilo docs "Settings" page (L1, via llms.txt), "CLI Runtime Architecture" page,
section "Config precedence" (L2), "CLI Config Schema" page (L3), "Agent Permissions"
page (L4), and Kilo's published schema [app.kilo.ai/config.json](https://app.kilo.ai/config.json)
(L5; draft 2020-12, `additionalProperties: false`, `$id` still
`https://opencode.ai/config.json` — L3 describes it as the upstream OpenCode schema
overlaid with Kilo's extras by Kilo's own route).

**Decision (brief's Kilo rule): rows, narrowly.** L5 resolves the values and L2 states
scalar merge ("Later sources override earlier values during instance config load").
Rows go only to scalar keys where L5 and the 7.8.18 runtime agree. L5 is ahead of
or apart from the runtime on several keys (see Live CLI check), and L3 says the
schema "does not load, apply, or override effective runtime config", so L5 alone is
not enough for a row.

## Claims

| # | Key (JSON pointer) | Value type and values | Merge (scalar / set / map; across which files) | Scope (global / project) | Category | Source URL | Verbatim quote | Page date / vendor version |
|---|---|---|---|---|---|---|---|---|
| 1 | (file) `~/.config/kilo/kilo.jsonc` | JSONC object | later sources override (L2 order: global files, `KILO_CONFIG`, project files, `KILO_CONFIG_DIR`, …); `kilo.json`/`opencode.json[c]`/`config.json` in the same place deep-merged | global (+ project) | — | L1, L2 | "Kilo reads JSONC config from a **global** location (`~/.config/kilo/kilo.jsonc`) and from your **project** (`kilo.jsonc`, or `.kilo/kilo.jsonc`)." · "If `kilo.json` or the legacy `opencode.json`, `opencode.jsonc`, or `config.json` files exist in the same locations, Kilo reads and deep-merges them as well." | none / 7.8.18 |
| 2 | `/model` | string `provider/model`, or null | scalar | global | behaviour default | L5, docs "Model Selection" page | "Model to use in the format of provider/model, eg anthropic/claude-2" · "the `model` key in `kilo.jsonc`" | none / 7.8.18 |
| 3 | `/small_model` | string `provider/model`, or null | scalar | global | behaviour default | L5 | "Small model to use for tasks like title generation in the format of provider/model" | none / 7.8.18 |
| 4 | `/default_agent` | string (agent name) | scalar | global | behaviour default | L5 | "Default primary agent to use when no session agent is selected" | none / 7.8.18 |
| 5 | `/reasoning_display` | enum `expanded`, `preview`, `headline` | scalar | global | behaviour default | L5 | "Controls how reasoning blocks are displayed in the VS Code chat UI" | none / 7.8.18 |
| 6 | `/terminal_command_display` | enum `expanded`, `collapsed` | scalar | global | behaviour default | L5 | "Controls whether terminal command blocks are expanded or collapsed by default in the VS Code chat UI" | none / 7.8.18 |
| 7 | `/code_edit_display` | enum `expanded`, `collapsed` | scalar | global | behaviour default | L5 | "Controls whether code edit and diff blocks are expanded or collapsed by default in the VS Code chat UI" | none / 7.8.18 |
| 8 | `/web_search` | bool | scalar | global | behaviour default | L5 | "Make web search available to models from all providers (default: false)" | none / 7.8.18 |
| 9 | `/hide_prompt_training_models` | bool | scalar | global | behaviour default | L5 | "Hide Kilo Gateway models that may train on your prompts from model listings" | none / 7.8.18 |
| 10 | `/privacy_mode` | bool | scalar | global | behaviour default | L5 | "Blur personally identifiable information (account email, balance, team name, etc.) in the TUI" | none / 7.8.18 |
| 11 | `/update` | enum `disable`, `notify`, `auto` | scalar | global | behaviour default | L5 | "Disable updates, notify when one is available, or install updates automatically" | none / 7.8.18 |
| 12 | `/autoupdate` | bool or `notify` | scalar | global | behaviour default | L5 | `"deprecated": true` · "true and 'notify' behave the same" | none / 7.8.18 |
| 13 | `/snapshots` vs `/snapshot` | bool | scalar | global | behaviour default | L5 | `snapshots`: "Enable snapshots used for undo and revert behavior"; `snapshot`: `"deprecated": true` | none / 7.8.18 |
| 14 | `/auto_expand_history`, `/warming` | bool (warming: bool or object) | scalar | global | behaviour default | L5 | "Automatically expand command history when searching" | none / 7.8.18 |
| 15 | `/share` | enum `manual`, `auto`, `disabled` | scalar | global | — | L5 | "Control whether sessions may be shared manually, automatically, or not at all" | none / 7.8.18 |
| 16 | `/remote_control` | bool | scalar | global | — | L5 | "Enable remote control of sessions via Kilo Cloud." | none / 7.8.18 |
| 17 | `/permission/<tool>` | `ask`/`allow`/`deny`, or a pattern map | order-dependent: last matching rule wins | global | permission policy | L4, L5 | "Permission rules are evaluated in config order. When more than one rule matches the requested permission and target pattern, the last matching rule wins." · L5: `"permission": {…, "deprecated": true}` | none / 7.8.18 |
| 18 | `/permissions`, `/policies` | ordered arrays of rule objects | ordered | global | permission policy | L5 | "Ordered tool permission rules applied to agent tool use" | none / 7.8.18 |
| 19 | arrays and objects (`/instructions`, `/plugin`, `/plugins`, `/mcp`, `/agent`, `/provider`, `/enabled_providers`, `/disabled_providers`, …) | arrays / objects | deep merge stated only for same-location files; cross-scope array merge unstated | global | — | L1, L2 | "Kilo reads and deep-merges them as well." | none / 7.8.18 |

## Verifier sign-off
none required

## Live CLI check

npm `@kilocode/cli@7.8.18` (`npm install --ignore-scripts --prefix <scratch>`); the
native binary from `@kilocode/cli-linux-x64` ran directly. All runs from a scratch
project dir: `env -i HOME=<sb> TMPDIR=<sb> XDG_CONFIG_HOME=<sb>/xdg PATH=<sb>/bin:/usr/bin:/bin DO_NOT_TRACK=1 KILO_DISABLE_AUTOUPDATE=1 timeout 60 kilo <verb> </dev/null`.

- `kilo --version` — pass — `7.8.18`.
- `kilo debug paths` — pass — `config  <sb>/xdg/kilo` (the global config dir follows `XDG_CONFIG_HOME`).
- `kilo debug config` ("show resolved configuration"; no login, no model) with `<sb>/xdg/kilo/kilo.jsonc` — **pass** for rows 2-10, 12, 13 (`snapshot`), 15, 16, 17: every value run resolved to the written value; every bad-value control exited 1 with a named diagnostic (e.g. `reasoning_display: "bogus"` → `Expected "expanded" | "preview" | "headline", got "bogus"`; malformed JSONC → "PropertyNameExpected"). A `//` comment is accepted.
- **fail** (schema and runtime disagree): `update`, `snapshots`, `auto_expand_history` and `warming` were dropped from the resolved config for good and bad values alike (control passed) — the 7.8.18 runtime does not read these L5 keys. It does read and validate `autoupdate` and `snapshot`, which L5 marks deprecated.

## Routing

- Harness: rows · re-check trigger: a Kilo CLI release whose runtime reads `update`/`snapshots`, or an L5 change; re-run `kilo debug config` with the controls.
- Settings file: `$XDG_CONFIG_HOME/kilo/kilo.jsonc`, default `~/.config/kilo/kilo.jsonc` (Windows `C:\Users\<username>\.config\kilo\kilo.jsonc`, L1) · JSONC · relocated by `XDG_CONFIG_HOME` (live); `KILO_CONFIG` and `KILO_CONFIG_DIR` add further sources (L2 order 5 and 7) rather than move the global file. Covered by the existing `kilo-config-root` (`vendor_kilo::kilo_config_root`); no new `VendorRoot` row. grim has never written `kilo.jsonc` (MCP is declined), so the file choice (`kilo.jsonc` vs an existing `kilo.json`) needs the OpenCode `config_path_for_scope` treatment.
- Limits for every row: a project `kilo.jsonc` or `.kilo/kilo.jsonc` overrides a global piece (L2); Kilo's Settings UI "removes it from every config file that contains it" when a user clears a setting (L1), which reads `missing` until the next install (Decision 11).
- Project scope: project `kilo.jsonc` / `.kilo/kilo.jsonc` exist — project scope: needs an ADR decision.
- Variable expansion: **expands, refuse at build** (added 2026-11-05 by the closing-review fix pass; follow-ups plan PD-35). Kilo resolves `{env:VAR}` and `{file:...}` in config strings, and the global file grim writes is the trusted location where both resolve. The docs corpus (re-fetched raw 2026-11-05), "Kilo Code CLI" page: "`{env:VAR}` (and `{file:...}`) references are resolved **only** in trusted config: your global config (`~/.config/kilo`), a config passed via `KILO_CONFIG` / `KILO_CONFIG_CONTENT`, or organization/MDM-managed config." The "Custom Models" page carries the same rule, and a project file resolves `{file:...}` only inside the project root. So a delivered string row (`/model`, `/small_model`, `/default_agent`) could read a user's variable or file into Kilo's config. grim refuses a Kilo value holding `{env:` or `{file:` at build (`holds an expansion token`, exit 65), as for OpenCode. Earlier source: the 2026-09-27 Kilo research, row I1 ([Kilo-Org/kilocode#14345](https://github.com/Kilo-Org/kilocode/pull/14345), "The config variable replacer matches `{env:...}` and `{file:...}` tokens anywhere in the text").
- Per key:
  - `/model`, `/small_model`, `/default_agent` → row (string, scalar, global)
  - `/reasoning_display`, `/terminal_command_display`, `/code_edit_display` → row (enum, scalar, global); they configure the VS Code extension, which reads the same file (L1)
  - `/web_search`, `/hide_prompt_training_models`, `/privacy_mode` → row (bool, scalar, global)
  - `/update`, `/autoupdate` → off: L5's current key is not read by the 7.8.18 runtime, and the key the runtime reads is deprecated in L5 (values unresolved between the two)
  - `/snapshots`, `/snapshot` → off: same split as `update`
  - `/auto_expand_history`, `/warming` → off: not read by the 7.8.18 runtime
  - `/share`, `/remote_control` → off: data-sharing and remote-access controls, outside the three categories
  - `/permission/<tool>` → off: deprecated in L5 in favour of `permissions`, and order-dependent (last match wins), which grim's map merge cannot place
  - `/permissions`, `/policies` → off: ordered lists of rule objects; no grim value type
  - arrays and objects → off: cross-scope array merge unstated; MCP, agents and providers are other kinds or out of the three categories

## Issue drafts
none (findings for the orchestrator go in your report)

## Friction

- The scoping URL `kilo.ai/docs/code-with-ai/platforms/cli` is a 404; the docs corpus is only reachable through `llms.txt` and `kilocode-docs.vercel.app/api/raw-markdown`.
- L5's `$id` still names opencode.ai, which makes the schema's provenance easy to misread.

## Skill changes
none
