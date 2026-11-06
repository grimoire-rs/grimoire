# Research: upstream refresh — Appendix B keys on the first six harnesses (deep, #270)

## Metadata

**Date:** 2026-11-05 · **Domain:** vendor AI-config layouts · **Depth:** deep (settings row)
**Triggered by:** /hex-execute .agents/plans/plan_settings_follow_ups.md (R-C)
**Expires:** 2027-05-05

Scope: every Appendix B item of `.agents/adr/adr_settings_artifact_kind.md` on Claude, Codex,
Copilot, Cursor, OpenCode and Qoder, plus the "Appendix B keys on the first six" table of
`.agents/research/research_settings_follow_ups_harness_scope.md`. A first-slice key that already
ships (`settings_fields()` in `src/install/vendor_{claude,codex,copilot,cursor,opencode,qoder}.rs`,
90 rows; `docs/src/content/docs/settings.md#fields` lists the same 90, checked per vendor: 13, 18,
19, 16, 9, 15) is not a candidate.

Method. Every page was fetched raw with `curl` (vendor `.md` endpoints, the github/docs raw
markdown, vendor JSON schemas), never through a summarising fetcher, so every quote is verbatim.
A script compared all 108 quote fragments with the saved pages: 102 match byte for byte after
whitespace folding. The other six differ only in markup the quotes drop: inline link syntax on
the Claude pages, `{% data variables… %}` product-name variables on the Copilot page (rendered as
"Copilot CLI" / "Copilot"), and `**` emphasis on the Qoder page.
No vendor page shows a page date; each row gives "no date shown, fetched 2026-11-05" plus the vendor
version checked. Live checks ran the npm-published binaries in scratch sandboxes (`env -i`, scratch
`HOME`/`TMPDIR`/`XDG_CONFIG_HOME`/vendor config dir, `PATH` = scratch bin + `/usr/bin:/bin`, stdin
`/dev/null`, `timeout 60`, `npm install --ignore-scripts`). No login, no model call, no session.
Accepted values were never taken from an error message's list.

grim's value types (`src/install/vendor.rs:570-640`): `Bool`, `String`, `Enum`, `StringList`,
`OneOf`, `Atomic{keys}` (first level closed, inner values checked for `null` and floats only),
`EnumMap`. There is **no number type**. Merge classes: `Set`, `Map`, `Scalar` (ordered lists and
atomic objects included), `OwnFile`.

## Summary

| Harness | Proposed rows | Off |
|---|---|---|
| Claude | 10: `/theme`, `/editorMode`, `/tui`, `/attribution/{commit,pr,sessionUrl}`, `/sandbox/filesystem/{allowWrite,denyWrite,denyRead,allowRead}` | none of the Appendix B keys |
| Codex | 4: `/file_opener`, `/tui/notification_method`, `/tui/notification_condition`, `/web_search` | granular `/approval_policy`, rules `match`/`not_match` |
| Copilot | 42: 7 Enum, 33 Bool (incl. `/sandbox/allowBypass`), 2 StringList Set (`/subagents/disabledSubagents`, `/enabledMcpServers`) | tool rules (managed-only), `/sandbox/userPolicy/*` (first-slice decision), `/tabs/hide`, `/tabs/sort`, `/footer/*`, numbers, credentials, egress |
| Cursor | 0 | `/sandbox/mode`, `/sandbox/networkAccess`, `/model` |
| OpenCode | 1: `/share` restricted to `manual`/`disabled` | `/username`, `/compaction/reserved`, `tui.json` keys |
| Qoder | 1: `/model/reasoningEffort` | none; array merge stays `on-limited` |

---

## Claude

Vendor version checked: `@anthropic-ai/claude-code` 2.1.318 (npm). Pages: [settings](https://code.claude.com/docs/en/settings.md),
[settings reference](https://code.claude.com/docs/en/settings-reference.md),
[sandboxing](https://code.claude.com/docs/en/sandboxing.md); no date shown, fetched 2026-11-05.

### Claims

| # | Key (JSON pointer) | Value type and values | Merge (across which files) | Scope | Category | Source URL | Verbatim quote | Page date / vendor version |
|---|---|---|---|---|---|---|---|---|
| CL-1 | `/theme` | string: `auto`, `dark`, `light`, `dark-daltonized`, `light-daltonized`, `dark-ansi`, `light-ansi`, or `custom:<slug>` / `custom:<plugin-name>:<slug>`; default `dark` | scalar; managed > command line > project local > shared project > user | Any file | behaviour default | settings-reference.md § `theme`; settings.md | "**Scope**: `Any file`. A value in `~/.claude.json` from an older version applies when no settings file sets it." / "`"custom:<slug>"` or `"custom:<plugin-name>:<slug>"`: a custom theme from `~/.claude/themes/` or a plugin" / settings.md: "A key set at a higher level overrides the same key set lower down." | no date; 2.1.318 |
| CL-2 | `/editorMode` | string: `normal`, `vim`; default `normal` | scalar, as CL-1 | Any file | behaviour default | settings-reference.md § `editorMode` | "**Type**: string, one of: `"normal"` … `"vim"`" / "Appears in `/config` as **Editor mode**, which writes this key to user settings." | no date; 2.1.318 |
| CL-3 | `/tui` | string: `default`, `fullscreen`; default unset | scalar, as CL-1; `CLAUDE_CODE_NO_FLICKER` and `CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN` win for one session | Any file | behaviour default | settings-reference.md § `tui` | "**Type**: string, one of: `"default"`: the classic main-screen renderer … `"fullscreen"`: the flicker-free alt-screen renderer" / "Running `/tui fullscreen` or `/tui default` writes this key for you." | no date; 2.1.318 |
| CL-4 | `/attribution/commit` | string; `""` hides commit attribution | scalar, as CL-1 | Any file | behaviour default | settings-reference.md § `attribution.commit` | "Set it to an empty string to hide commit attribution. … **Type**: string" | no date; 2.1.318 |
| CL-5 | `/attribution/pr` | string; `""` hides PR attribution | scalar, as CL-1 | Any file | behaviour default | settings-reference.md § `attribution.pr` | "Set it to an empty string to hide pull request attribution. … **Type**: string" | no date; 2.1.318 |
| CL-6 | `/attribution/sessionUrl` | Boolean; default `true` | scalar, as CL-1 | Any file | behaviour default | settings-reference.md § `attribution.sessionUrl` | "**Type**: Boolean … **Default**: `true`" | no date; 2.1.318 |
| CL-6a | `/attribution` (parent) | object with the three sub-keys, **or `false`** | — (container) | Any file | — | settings-reference.md § `attribution` | "object with `commit` and `pr` strings and a `sessionUrl` Boolean, or `false` to hide all attribution. The `false` value requires Claude Code v2.1.281 or later; earlier versions reject it and skip the whole user, project, or local settings file that holds it" | no date; 2.1.318 |
| CL-7 | `/sandbox/filesystem/allowWrite` | array of path strings (`/`, `~/`, `./` prefixes) | **set: combined across files** unless a managed lock covers the entry | Any file, "with limits on project and local settings" | permission policy | sandboxing.md; settings-reference.md | sandboxing.md: "When you define the same filesystem array in multiple settings scopes, Claude Code merges them, combining the paths rather than replacing one scope's array with another's. Claude Code leaves an entry out of the merge when a lock under Keep developers from widening the policy covers it." / reference: "Unless a lock applies, Claude Code merges these lists across the settings files the session loads." | no date; 2.1.318 |
| CL-8 | `/sandbox/filesystem/denyWrite` | array of path strings | set, as CL-7 | Any file | permission policy | as CL-7 | as CL-7; "**Scope**: `Any file`" | no date; 2.1.318 |
| CL-9 | `/sandbox/filesystem/denyRead` | array of path strings | set, as CL-7 | Any file | permission policy | as CL-7 | as CL-7 | no date; 2.1.318 |
| CL-10 | `/sandbox/filesystem/allowRead` | array of path strings | set, as CL-7 | Any file, with limits on project and local settings | permission policy | as CL-7 | "**Scope**: `Any file`, with limits on project and local settings" | no date; 2.1.318 |

Path resolution caution for CL-7 to CL-10 (reference § Sandbox path prefixes): "`./` or no
prefix | Relative to the project root for project settings, or to `~/.claude` for user settings".
The same relative entry means a different path at grim's two scopes.

### Live CLI check

`claude doctor` (native binary `node_modules/@anthropic-ai/claude-code-linux-x64/claude`) ran
without a login; its help says it "Reads settings files in the current directory without a trust
prompt". It always exits 0; a rejected value shows as an `Invalid settings - <file> › <pointer>` line.

- `env -i PATH=… HOME=… TMPDIR=… XDG_CONFIG_HOME=… CLAUDE_CONFIG_DIR=… DISABLE_TELEMETRY=1 DISABLE_ERROR_REPORTING=1 CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 DO_NOT_TRACK=1 timeout 60 claude doctor` with malformed JSON in user `settings.json` — pass — "Invalid or malformed JSON" reported, so doctor reads the file.
- same, `tui` = `default`, `fullscreen` — pass — both clean; control `"windowed"` reported as "Invalid value".
- same, `editorMode` = `vim` — skip — control `"emacs"` was not reported, so doctor does not check this value.
- same, `theme` = `light-ansi`, `custom:my-theme` — skip — control `"Dark"` was not reported.
- same, `attribution` = `{commit: "", pr: "…", sessionUrl: false}` and `attribution: false` — pass — clean; controls `commit: 5` and `sessionUrl: "no"` reported with the expected type. An unknown sub-key (`bogus`) was not reported.
- same, the four `sandbox.filesystem` arrays — pass (types) — clean; controls (a string instead of an array, a number element) reported. Cross-file merge: not run — it needs a session; the merge rests on the two quoted pages.

### Routing

- Harness: rows · re-check trigger: a Claude Code release that changes the `theme` built-in list, adds a `tui` value, or changes the `attribution` shape.
- Settings file: `~/.claude/settings.json` (Linux, macOS) · strict JSON · `CLAUDE_CONFIG_DIR`. Project scope: grim's existing route to the main-checkout `.claude/settings.local.json`.
- `/theme` → row (`Enum[auto, dark, light, dark-daltonized, light-daltonized, dark-ansi, light-ansi]`, Scalar, both). `custom:` themes are not writable: they name a file the package cannot ship. A user's custom value stays a valid current value.
- `/editorMode` → row (`Enum[normal, vim]`, Scalar, both).
- `/tui` → row (`Enum[default, fullscreen]`, Scalar, both).
- `/attribution/commit` → row (String, Scalar, both).
- `/attribution/pr` → row (String, Scalar, both).
- `/attribution/sessionUrl` → row (Bool, Scalar, both). For all three: a user's `attribution: false` is a non-object at the container, so a grim write there meets a type conflict and must refuse, never replace `false`.
- `/sandbox/filesystem/allowWrite` → row (StringList, Set, both), `on-limited`: limited on project and local settings (the limit links to sandboxing.md § "Repository settings under an admin-required sandbox"), and relative paths resolve per file.
- `/sandbox/filesystem/denyWrite` → row (StringList, Set, both).
- `/sandbox/filesystem/denyRead` → row (StringList, Set, both).
- `/sandbox/filesystem/allowRead` → row (StringList, Set, both), `on-limited` as `allowWrite`.

---

## Codex

Vendor version checked: `@openai/codex` 0.166.0 (npm). Pages: [config reference](https://learn.chatgpt.com/docs/config-file/config-reference.md),
[config schema](https://learn.chatgpt.com/docs/config-schema.json),
[rules](https://learn.chatgpt.com/docs/agent-configuration/rules.md); no date shown, fetched 2026-11-05.

### Claims

| # | Key (JSON pointer) | Value type and values | Merge (across which files) | Scope | Category | Source URL | Verbatim quote | Page date / vendor version |
|---|---|---|---|---|---|---|---|---|
| CX-1 | `/file_opener` | `vscode`, `vscode-insiders`, `windsurf`, `cursor`, `none`; default `vscode` | scalar; user `~/.codex/config.toml`, project `.codex/config.toml` only when trusted | global (grim) | behaviour default | config-reference.md; config-schema.json | `type: "vscode \| vscode-insiders \| windsurf \| cursor \| none"`, "URI scheme used to open citations from Codex output (default: `vscode`)." | no date; 0.166.0 |
| CX-2 | `/tui/notification_method` | `auto`, `osc9`, `bel`; default `auto` | scalar | global | behaviour default | as CX-1 | `type: "auto \| osc9 \| bel"`, "Notification method for terminal notifications (default: auto)." | no date; 0.166.0 |
| CX-3 | `/tui/notification_condition` | `unfocused`, `always`; default `unfocused` | scalar | global | behaviour default | as CX-1 | `type: "unfocused \| always"`, "Control whether TUI notifications fire only when the terminal is unfocused or regardless of focus. Defaults to `unfocused`." | no date; 0.166.0 |
| CX-4 | `/web_search` | `disabled`, `cached`, `indexed`, `live`; default `cached` (`live` under full access) | scalar; an admin requirement can narrow the allowed set | global | permission policy | as CX-1 | `type: "disabled \| cached \| indexed \| live"`, "Web search mode (default: `"cached"`; … if you use `--yolo` or another full access sandbox setting, it defaults to `"live"`)." / requirements: "`disabled` is always allowed" | no date; 0.166.0 |
| CX-5 | `/approval_policy` (granular form) | union: `on-request` \| `never` \| `{ granular = { sandbox_approval, rules, mcp_elicitations, request_permissions, skill_approval } }` (bools); schema requires `mcp_elicitations`, `rules`, `sandbox_approval` | scalar | global | permission policy | as CX-1 | `type: "on-request \| never \| { granular = { sandbox_approval = bool, rules = bool, mcp_elicitations = bool, request_permissions = bool, skill_approval = bool } }"` | no date; 0.166.0 |
| CX-6 | rules file `match`, `not_match` | lists of example command strings; default `[]` | own file (`~/.codex/rules/grim-*.rules`) | global | permission policy | rules.md | "`match` and `not_match` (defaults to `[]`): Examples that Codex validates when it loads your rules. Use these to catch mistakes before a rule takes effect." | no date; 0.166.0 |

Settings-file facts: "User-level configuration lives in `~/.codex/config.toml`. You can also add
project-scoped overrides in `.codex/config.toml` files. Codex loads project-scoped config files only
when you trust the project."

### Live CLI check

All runs: `env -i PATH=… HOME=… TMPDIR=… XDG_CONFIG_HOME=… CODEX_HOME=… DO_NOT_TRACK=1 timeout 60 codex -c analytics.enabled=false -c feedback.enabled=false -c check_for_update_on_startup=false debug prompt-input`, with the value in `$CODEX_HOME/config.toml`. Baseline (empty file) exit 0, so the verb is credential-free.

- `file_opener` = each of the five values — pass — all exit 0; controls `"VSCode"`, `"zed"` exit 1 (config load rejected).
- `web_search` = each of the four values — pass — exit 0; controls `"Live"`, `"off"` exit 1.
- `tui.notification_method` = `auto`, `osc9`, `bel` — pass — exit 0; controls `"OSC9"`, `"desktop"` exit 1.
- `tui.notification_condition` = `unfocused`, `always` — pass — exit 0; controls `"Always"`, `"never"` exit 1.
- `approval_policy` granular — pass (behaviour recorded) — the three required bools: exit 0; all five: exit 0; only `sandbox_approval`: exit 1 (`missing field rules`, whole config rejected); a string value for a bool: exit 1; a second key beside `granular`: exit 1; bare `"granular"`: exit 1; **a misspelled inner key (`sandbox_aproval`) beside the required three: exit 0, silently ignored**. Control `approval_policy = "bogus"` exit 1.
- `codex execpolicy check --rules <file> -- rm -rf build` with correct `match`/`not_match` — pass — exit 0, decision `forbidden`. A `match` example the pattern misses: exit 1, "failed to parse policy … expected every example to match at least one rule". A `not_match` example the pattern hits: exit 1. **With a second, valid rules file passed beside the failing one, the check still exits 1 and evaluates nothing**: one wrong example fails the whole policy load. Session-time behaviour is not run (needs a session).

### Routing

- Harness: rows · re-check trigger: a Codex release that changes any of the four enums, or a config-reference change to `approval_policy`.
- Settings file: `~/.codex/config.toml` (Linux, macOS) · TOML · `CODEX_HOME`. Project file loads only for trusted projects; grim stays global.
- `/file_opener` → row (`Enum[vscode, vscode-insiders, windsurf, cursor, none]`, Scalar, global).
- `/tui/notification_method` → row (`Enum[auto, osc9, bel]`, Scalar, global).
- `/tui/notification_condition` → row (`Enum[unfocused, always]`, Scalar, global).
- `/web_search` → row (`Enum[disabled, cached, indexed, live]`, Scalar, global). `live` loosens; an admin requirement can refuse a value.
- `/approval_policy` granular → off: grim's types express the outer union (`OneOf[Enum[on-request, never], Atomic{keys: [granular]}]`), but not the inner table. A missing required inner key or a non-bool fails Codex's whole config load (verified), and `Atomic` checks neither. A misspelled inner key is silently ignored by Codex (verified), so a typo goes unseen. Widening the shipped `Enum` row would also change a first-slice row.
- rules `match`, `not_match` → off: one wrong example fails the whole policy load, other rules files included (verified with `execpolicy check`). grim's `Atomic{pattern, decision, justification}` cannot check an example against its pattern. It could ship only if `grim build` evaluated the examples: new code, an owner decision.

---

## Copilot

Vendor version checked: `@github/copilot` 1.0.105 (npm). Page: [CLI configuration directory reference](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference),
read **to its end** as raw markdown from
`github/docs` `content/copilot/reference/copilot-cli-reference/cli-config-dir-reference.md`
(1048 lines, 100,930 bytes; last commit to the file c5e62c4c9a, 2026-11-03). Also
`copilot help config` from the 1.0.105 binary.

Merge facts used by every row below:

- Order: "Settings are applied in this order (later overrides earlier): 1. Built-in defaults 2. Mobile Device Management (MDM) managed settings 3. User settings (`~/.copilot/settings.json`) 4. Repository settings (`.github/copilot/settings.json`) 5. Local settings (`.github/copilot/settings.local.json`) 6. Environment variables 7. Command-line flags".
- Repository layer: "Only the keys listed in the following table are supported at the repository level. Any other keys—including keys that are valid in the user configuration file—are silently ignored." None of the keys below is in that table, so each has one file: the user file.
- Sandbox: "Sandbox settings are user-level or enterprise-managed. They are not repository-overridable." and "each managed `sandbox` value sets a floor that you cannot relax."
- Bad values: "If `settings.json` fails to read, parse, or validate, Copilot CLI ignores the invalid values and shows a startup warning".

So every `Scalar` row below merges as "user file only, later layers override", and every `Set`
row is owned within the one user file.

### Claims

Source for every row: the config-dir reference, user settings table (§ "User settings
(`~/.copilot/settings.json`)"), unless noted. Page date 2026-11-03 (repo commit); vendor 1.0.105.
Quotes are the table's Type cell plus the clause that matters.

| # | Key (JSON pointer) | Value type and values | Merge | Scope | Category | Verbatim quote |
|---|---|---|---|---|---|---|
| CP-1 | `/editorMode` | `normal`, `vim`; default `normal` | scalar, user only | global | behaviour default | "`"normal"` \| `"vim"` … Input composer editing mode." |
| CP-2 | `/autoTier` | `efficiency`, `balance`, `intelligence` | scalar, user only | global | behaviour default | "`"efficiency"` \| `"balance"` \| `"intelligence"` … `"fast"` is no longer selectable" |
| CP-3 | `/banner` | `always`, `once`, `never` | scalar | global | behaviour default | "`"always"` \| `"once"` \| `"never"` … Animated banner display frequency." |
| CP-4 | `/keepAlive` | `on`, `off`, `busy` | scalar | global | behaviour default | "`"on"` \| `"off"` \| `"busy"` … Keep-alive mode applied at CLI startup." |
| CP-5 | `/worktreeBaseRef` | `head`, `defaultBranch` | scalar | global | behaviour default | "`"head"` \| `"defaultBranch"` … Starting point for new worktrees" |
| CP-6 | `/autoUpdatesChannel` | `stable`, `prerelease` | scalar | global | behaviour default | "`"stable"` \| `"prerelease"` … Update channel." |
| CP-7 | `/logLevel` | `none`, `error`, `warning`, `info`, `debug`, `all`, `default` | scalar | global | behaviour default | "`"none"` \| `"error"` \| `"warning"` \| `"info"` \| `"debug"` \| `"all"` \| `"default"` … Logging verbosity." |
| CP-8 | `/stayInAutopilot` | boolean | scalar | global | behaviour default | "`boolean` \| `true` \| Remain in autopilot mode after each task completes." |
| CP-9 | `/continueOnAutoMode` | boolean | scalar | global | behaviour default | "Automatically switch to auto mode when rate-limited." |
| CP-10 | `/showReasoning` | boolean | scalar | global | behaviour default | "Show model reasoning in the timeline." |
| CP-11 | `/experimental` | boolean | scalar | global | behaviour default | "Enable experimental features." |
| CP-12 | `/toolSearch` | boolean | scalar | global | behaviour default | "Controls tool search (deferred tool loading)." |
| CP-13 | `/connectors` | boolean | scalar | global | permission policy | "Enable Copilot Connectors when available. Set to `false` to disable." |
| CP-14 | `/customAgents/defaultLocalOnly` | boolean | scalar | global | behaviour default | "Only use local custom agents (no remote organization or enterprise agents)." |
| CP-15 | `/autoUpdate` | boolean | scalar | global | behaviour default | "Automatically download CLI updates and update first-party plugins" |
| CP-16 | `/beep` | boolean | scalar | global | behaviour default | "Play an audible beep when attention is required." |
| CP-17 | `/beepOnSchedule` | boolean | scalar | global | behaviour default | "Play an audible beep when a scheduled `/every` or `/after` run finishes." |
| CP-18 | `/compactPaste` | boolean | scalar | global | behaviour default | "Collapse large pastes (more than 10 lines) into compact tokens." |
| CP-19 | `/copyOnSelect` | boolean (default per OS) | scalar | global | behaviour default | "`true` (macOS), `false` (other)" |
| CP-20 | `/inlineImages` | boolean | scalar | global | behaviour default | "Render images inline using the Kitty graphics protocol." |
| CP-21 | `/mouse` | boolean | scalar | global | behaviour default | "Enable mouse support." |
| CP-22 | `/pinnedPrompts` | boolean | scalar | global | behaviour default | "CLI UI only—has no effect on prompts sent to the model." |
| CP-23 | `/renderHexColors` | boolean | scalar | global | behaviour default | "Show six-digit hex color codes … as color swatches." |
| CP-24 | `/screenReader` | boolean | scalar | global | behaviour default | "Enable screen reader optimizations." |
| CP-25 | `/scrollbar` | boolean | scalar | global | behaviour default | "Show the scrollbar in scrollable views." |
| CP-26 | `/shellShortcut` | boolean | scalar | global | behaviour default | "User- or managed-scoped only—not repo-overridable." |
| CP-27 | `/showTimestamps` | boolean | scalar | global | behaviour default | "Show dim `HH:mm` timestamps next to user messages" |
| CP-28 | `/showTipsOnStartup` | boolean | scalar | global | behaviour default | "Show a random command tip when the CLI starts." |
| CP-29 | `/sidebar` | boolean | scalar | global | behaviour default | "Enable the current-session sidebar." |
| CP-30 | `/streamerMode` | boolean | scalar | global | behaviour default | "Hide preview model names, quota details, prompt timestamps" |
| CP-31 | `/terminalNotifications` | boolean | scalar | global | behaviour default | "Prefer terminal-owned OSC 777 notifications" |
| CP-32 | `/terminalProgress` | boolean | scalar | global | behaviour default | "Emit OSC 9;4 terminal progress indicators" |
| CP-33 | `/updateTerminalTitle` | boolean | scalar | global | behaviour default | "Show the current intent in the terminal tab or window title." |
| CP-34 | `/taskbarPresence` | boolean (Windows only) | scalar | global | behaviour default | "Startup-only; takes effect on the next launch. Windows only." |
| CP-35 | `/ide/autoConnect` | boolean | scalar | global | behaviour default | "Automatically connect to an IDE workspace on startup." |
| CP-36 | `/ide/openDiffOnEdit` | boolean | scalar | global | behaviour default | "Open file edit diffs in the connected IDE for approval." |
| CP-37 | `/dynamicRetrieval/skills` | boolean | scalar | global | behaviour default | "`{ skills?: boolean }` … Set `skills` to `false` to disable retrieval for skills." |
| CP-38 | `/tabs/enabled` | boolean | scalar | global | behaviour default | "Show the home tab bar." |
| CP-39 | `/tabs/hide` | string[] of tab ids (`copilot`, `agents`, `issues`, `pull-requests`, `gists`) | set, user only | global | behaviour default | "Accepted values: `"copilot"`, `"agents"`, `"issues"`, `"pull-requests"`, `"gists"` (matched case-insensitively)." help: "Unknown identifiers are ignored" |
| CP-40 | `/tabs/sort` | ordered string[] of tab ids | scalar (order matters) | global | behaviour default | "Order in which tabs are displayed. … Unknown identifiers are ignored." |
| CP-41 | `/footer/<item>` | boolean per item; items `showModelEffort`, `showDirectory`, `showBranch`, `showContextWindow`, `showQuota`, `showAgent`, `showAiUsed`, `showCodeChanges`, `showUsername`, `showSandbox`, `showYolo`, `showCustom` | map with closed keys | global | behaviour default | "Sub-keys include `showModelEffort`, … and `showCustom` (all `boolean`). Managed by the `/statusline` slash command." |
| CP-42 | `/subagents/disabledSubagents` | string[] | set, user only | global | behaviour default | "Names of subagents that are turned off." |
| CP-43 | `/subagents/agents/rubber-duck/autoInvoke` | boolean | scalar | global | behaviour default | "Allow Copilot to proactively consult the rubber duck agent without being asked." |
| CP-44 | `/enabledMcpServers` | string[] (built-in server names) | set, user only | global | permission policy | "Enable built-in MCP servers that are disabled by default." |
| CP-45 | `/sandbox/allowBypass` | boolean; default `true` | scalar; managed floor | global | permission policy | "Allow sandboxed commands to request a bypass for specific operations … Set to `false` to opt out." |
| CP-46 | `/sandbox/userPolicy/network/allowLocalNetwork` | boolean | scalar; managed floor | global | permission policy | "Control local network access" |
| CP-47 | `/sandbox/userPolicy/network/allowedHosts` | string[] (hostnames, `*.example.com`) | set; managed floor | global | permission policy | "Hosts a sandboxed command is allowed to reach." |
| CP-48 | `/sandbox/userPolicy/network/blockedHosts` | string[] | set; managed floor | global | permission policy | "`blockedHosts` always takes precedence over a matching `allowedHosts` entry" |
| CP-49 | `/sandbox/userPolicy/filesystem/deniedPaths` | string[] (paths) | set; managed: "denied paths can only be added to" | global | permission policy | "Missing denied paths and their parents are created as directories before launch and are not automatically removed" |
| CP-50 | tool rules `permissions.allow` / `ask` / `deny` / `limitTo` | rule strings (`Shell(...)`, `Read(...)`, `Edit(...)`, `Write(...)`, `Domain(...)`) | managed sources only | managed | permission policy | § Managed permission rules: "Add `deny`, `ask`, and `allow` rule lists under the managed `permissions` key"; `permissions-config.json` "doesn't support deny rules, "ask" rules, default modes, URL rules" |
| CP-51 | `/subagents/agents/<name>` | object `{model, modelPolicy: preferred\|required, effortLevel, contextTier: default\|long_context\|inherit}` | map, user only | global | behaviour default | "Each value is an object with optional `model` (string), `modelPolicy` (`"preferred"` or `"required"`), `effortLevel` (string), and `contextTier` …" |
| CP-52 | `/commandHistoryMaxSize`, `/subagents/maxConcurrency`, `/subagents/maxDepth` | number | scalar | global | behaviour default | "`number` \| `50` … Must be an integer between `1` and `1000`." |
| CP-53 | `/contextTier` | `default` \| `long_context` | repository table: "Replaced—repository takes precedence"; absent from the user table | repo/managed | behaviour default | "Pin the default context tier." (repository table) |
| CP-54 | `/remoteSessions`, `/remoteExport` | boolean | scalar | global | (egress) | "Sync session history to your account when session sync is available." |
| CP-55 | `/worktreePathTemplate`, `/skillDirectories`, `/ignoredSkillsLocations` | string / string[] paths | scalar / set | global | (paths) | "Where `/worktree`, `/move`, `/new`, and `--worktree` create worktrees" |
| CP-56 | `trustedFolders` (in `config.json`, not `settings.json`) | string[] of directories | — (internal state the CLI manages) | global | trust gate | [configure-copilot-cli](https://docs.github.com/en/copilot/how-tos/copilot-cli/set-up-copilot-cli/configure-copilot-cli), last commit 8e7396e5e2, 2026-10-28: "If you choose to trust the directory for future sessions, the trusted directory prompt will not be displayed again." / "Trusted directories are stored in the automatically managed `config.json` file." / "Edit the contents of the `trustedFolders` array." Config dir reference: "That file is used to store internal state, such as installed plugins and trusted folders." |

Keys on the page and out of scope by the field list's method note (one line, no row each):
`hooks`, `disableAllHooks`, `statusLine` (hooks, helper command); `bashEnv` (env);
`powershellFlags` (shell flags); `proxyUrl`, `proxyKerberosServicePrincipal`,
`sandbox.userPolicy.network.proxy` (endpoints, password); `storeTokenPlaintext`,
`sandbox.credentials`, `sandbox.auth.git`, `sandbox.auth.gh`,
`sandbox.userPolicy.seatbelt.keychainAccess` (credentials); `enabledPlugins` (kept off by decision);
`colorMode` (deprecated alias of the shipped `/theme`); `companyAnnouncements` (free text, not a
default). Keys in `copilot help config` 1.0.105 but **not** on the reference page: `bannerStyle`,
`notifications`, `memory`, `inlineImageLiveWindow`, `trustedFolders`. The last is documented
elsewhere, as state in `config.json` (CP-56, added 2026-11-05 by the closing-review fix pass).

### Live CLI check

All runs: `env -i PATH=… HOME=… TMPDIR=… XDG_CONFIG_HOME=… COPILOT_HOME=… COPILOT_OFFLINE=true COPILOT_AUTO_UPDATE=false DO_NOT_TRACK=1 timeout 60 copilot config <key> <value>` (native binary `node_modules/@github/copilot-linux-x64/copilot`). Its help: "Values must match the setting's type … or an accepted choice for enums." Writes go to the scratch `$COPILOT_HOME/settings.json`.

- `copilot --version`, `copilot help config` — pass — exit 0 (1.0.105), no login.
- `editorMode vim`, `editorMode normal` — pass — exit 0; control `emacs` exit 1.
- `banner never` — pass — exit 0; control `sometimes` exit 1.
- `keepAlive busy` — pass — exit 0; control `maybe` exit 1.
- `worktreeBaseRef defaultBranch` — pass — exit 0; control `main` exit 1.
- `autoTier balance` — pass — exit 0; control `bogus-tier` exit 1.
- `stayInAutopilot off` — pass — exit 0, stored `false`; control `perhaps` exit 1.
- `sandbox.allowBypass false` — pass — exit 0; control `sometimes` exit 1.
- `sandbox.userPolicy.network.allowedHosts example.com`, `sandbox.userPolicy.filesystem.deniedPaths /tmp/x-denied` — pass — exit 0, appended as array items (no control: a free string has no bad value).
- `tabs.hide issues` — pass — exit 0; control `bogus` also exit 0 — skip for element values (matches "Unknown identifiers are ignored").
- `permissions.allow 'Shell(git status)'`, `permissions.deny 'Shell(rm *)'` — pass — exit 1: "Setting "permissions.allow" is controlled by your organization's policy, not by your settings." Tool rules are not a user setting.
- `subagents.disabledSubagents rubber-duck` — skip — exit 1: "cannot be changed with `copilot config`. Edit settings.json directly."
- `bogusKey value` — pass (control) — exit 1, unknown key.

### Routing

- Harness: rows · re-check trigger: a change to the config-dir reference (watch commits to the github/docs file) or a Copilot CLI minor release.
- Settings file: `~/.copilot/settings.json` (Linux, macOS; Windows under `%USERPROFILE%\.copilot`) · JSONC · `COPILOT_HOME`. Repository and local files exist; Copilot stays global-only (ADR Decision 3).
- `/editorMode` → row (`Enum[normal, vim]`, Scalar, global).
- `/autoTier` → row (`Enum[efficiency, balance, intelligence]`, Scalar, global). `copilot help config` adds that "server-advertised profiles are also accepted when dynamic Auto tiers are enabled", so a user's value outside the three stays a valid current value.
- `/banner` → row (`Enum[always, once, never]`, Scalar, global).
- `/keepAlive` → row (`Enum[on, off, busy]`, Scalar, global).
- `/worktreeBaseRef` → row (`Enum[head, defaultBranch]`, Scalar, global).
- `/autoUpdatesChannel` → row (`Enum[stable, prerelease]`, Scalar, global).
- `/logLevel` → row (`Enum[none, error, warning, info, debug, all, default]`, Scalar, global).
- `/stayInAutopilot`, `/continueOnAutoMode`, `/showReasoning`, `/experimental`, `/toolSearch`, `/connectors`, `/customAgents/defaultLocalOnly`, `/autoUpdate`, `/beep`, `/beepOnSchedule`, `/compactPaste`, `/copyOnSelect`, `/inlineImages`, `/mouse`, `/pinnedPrompts`, `/renderHexColors`, `/screenReader`, `/scrollbar`, `/shellShortcut`, `/showTimestamps`, `/showTipsOnStartup`, `/sidebar`, `/streamerMode`, `/terminalNotifications`, `/terminalProgress`, `/updateTerminalTitle`, `/taskbarPresence`, `/ide/autoConnect`, `/ide/openDiffOnEdit`, `/dynamicRetrieval/skills`, `/tabs/enabled`, `/subagents/agents/rubber-duck/autoInvoke` → row each (Bool, Scalar, global). 32 rows; the cosmetic ones follow the Claude `/spinnerTipsEnabled` precedent and can be cut without loss.
- `/tabs/hide` → off: the elements form a closed set of five ids that grim's `StringList` cannot check, and Copilot ignores unknown ids silently (`copilot config tabs.hide bogus` exit 0), so a typo ships unseen.
- `/tabs/sort` → off: an ordered list over the same unchecked id set.
- `/footer/<item>` → off: "Sub-keys include …" does not claim the list is complete, so a closed `map_keys` is unsourced; and `/statusline` manages the object.
- `/subagents/disabledSubagents` → row (StringList, Set, global).
- `/enabledMcpServers` → row (StringList, Set, global); loosens, like the shipped `/allowedUrls`.
- `/sandbox/allowBypass` → row (Bool, Scalar, global); a managed value is a floor.
- `/sandbox/userPolicy/network/allowLocalNetwork`, `/allowedHosts`, `/blockedHosts`, `/sandbox/userPolicy/filesystem/deniedPaths` → off: the first slice kept `/sandbox/userPolicy/*` off by decision (field list § 1.3, "network hosts"). The facts above are verified if the owner reverses it (Bool Scalar; StringList Set). `deniedPaths` also creates missing directories before launch.
- Tool rules (`permissions.allow`, `ask`, `deny`, `limitTo`) → off: managed sources only (MDM, server); `copilot config permissions.allow …` is refused as organization policy (verified). This closes the Appendix B item: no user or repository key exists.
- `/subagents/agents/<name>` → off: `Atomic` cannot check the inner enums (`modelPolicy`, `contextTier`).
- `/commandHistoryMaxSize`, `/subagents/maxConcurrency`, `/subagents/maxDepth` → off: number values; grim has no number type.
- `/contextTier` → off: the reference lists it only in the repository and managed tables; `copilot help config` lists it as a user setting with example values ("e.g."). The pages disagree, and grim is global-only here.
- `/remoteSessions`, `/remoteExport` → off: remote control and session-history upload. Data egress outside the three categories, the same reason telemetry is scoped out.
- `/worktreePathTemplate`, `/skillDirectories`, `/ignoredSkillsLocations` → off: filesystem locations and content discovery paths, outside the three categories.
- `bannerStyle`, `notifications`, `memory`, `inlineImageLiveWindow` → off: in `copilot help config` only, not on the reference page; re-check when the page lists them.
- `trustedFolders` → off for good, no re-check trigger (CP-56, 2026-11-05): it is the CLI's record of permanently trusted directories, kept in the automatically managed `config.json`, not in the `settings.json` grim edits. A write there would pre-trust a folder and skip the trust prompt, which threat model T3 rules out (the trust-gate rule, follow-ups plan PD-41; Antigravity `/trustedWorkspaces` is off for the same reason).

---

## Cursor

Pages: [CLI configuration](https://cursor.com/docs/cli/reference/configuration.md),
[run modes](https://cursor.com/docs/agent/security/run-modes.md),
[CLI parameters](https://cursor.com/docs/cli/reference/parameters.md),
[CLI changelog](https://cursor.com/docs/cli/changelog.md); no date shown on the reference pages,
fetched 2026-11-05 (changelog head v2026.10.22). No vendor binary run (see below).

### Claims

| # | Key (JSON pointer) | Value type and values | Merge | Scope | Category | Source URL | Verbatim quote | Page date / vendor version |
|---|---|---|---|---|---|---|---|---|
| CU-1 | `/sandbox/mode` | string; **values not listed for the key** | not stated | global ("Only permissions can be configured at the project level") | permission policy | configuration.md | "`sandbox.mode` \| string \| Sandbox mode override". The only value list is for a CLI flag: parameters.md "`--sandbox <mode>` \| Set sandbox mode: `enabled` or `disabled`" | no date; n/a |
| CU-2 | `/sandbox/networkAccess` | string; **values not listed for the key** | not stated | global | permission policy | configuration.md; run-modes.md | "`sandbox.networkAccess` \| string \| Network access setting for sandbox mode". run-modes.md names three modes by display label only: "**sandbox.json Only**", "**sandbox.json + Defaults**", "**Allow All**" | no date; n/a |
| CU-3 | `/model` | object; **shape not given** | not stated; CLI-managed | global | behaviour default | configuration.md | "`model` \| object \| Selected model configuration" / "`hasChangedDefaultModel` \| boolean \| CLI-managed model override flag" / "Some fields are CLI-managed and may be overwritten." | no date; n/a |
| CU-4 | `autoAcceptWebSearch` (pointer not stated) | **type not given**; a toggle, off by default | not stated | global | permission policy | changelog.md § v2026.07.13 › Approvals and trust; configuration.md and permissions.md re-fetched 2026-11-05 | "A new Auto-Accept Web Search permission lets the agent run web searches without pausing for approval each time. It is off by default. Toggle it under Permissions in `/config` or set `autoAcceptWebSearch` in `cli-config.json`." Neither the configuration reference nor the permissions page names the key | changelog heading v2026.07.13; added 2026-11-05 by the closing-review fix pass |

### Live CLI check

- Cursor CLI — not run: no Cursor-published npm package (npm `cursor-agent` 1.0.3 is a third-party "Task sequence creator for Cursor AI agents" by `zalab-inc`); the documented install is `curl https://cursor.com/install -fsS | bash`, which the rules forbid; no archive with a published checksum was found. A value for `sandbox.mode` or `sandbox.networkAccess` could only come from an error message, which the claim rules forbid anyway.

### Routing

- Harness: no rows from Appendix B · re-check trigger: the configuration page lists values for `sandbox.mode` or `sandbox.networkAccess`, or a shape for `model`.
- Settings file: unchanged from the first slice (`$CURSOR_CONFIG_DIR/cli-config.json`, else `~/.cursor/cli-config.json`, else `$XDG_CONFIG_HOME/cursor/cli-config.json`) · pure JSON · `CURSOR_CONFIG_DIR`.
- `/sandbox/mode` → off: no value set for the key; the flag's `enabled`/`disabled` is documented for `--sandbox`, not for the stored key.
- `/sandbox/networkAccess` → off: the three modes have display labels only, no stored strings.
- `/model` → off: object shape not given, and the CLI manages it.
- `autoAcceptWebSearch` → off (CU-4, 2026-11-05): only the changelog names it, with no type and no pointer (top level or nested), and the configuration reference does not list it. No Cursor binary runs credential-free (Live CLI check above), so no value can be tested. It is a loosening permission (web searches without approval). Re-check trigger: the configuration reference lists the key with a type.

---

## OpenCode

Vendor version checked: `opencode-ai` 1.19.5 (npm `latest`, published 2026-10-28). Sources:
[config schema](https://opencode.ai/config.json) (no Last-Modified header; ETag
`a07cdc79fa7e210ef70ba4ade80a6b1f`; fetched 2026-11-05),
[tui schema](https://opencode.ai/tui.json), [config docs](https://opencode.ai/docs/config.md),
[permissions docs](https://opencode.ai/docs/permissions.md), [tui docs](https://opencode.ai/docs/tui.md);
no date shown on any docs page.

### Claims

| # | Key (JSON pointer) | Value type and values | Merge | Scope | Category | Source URL | Verbatim quote | Page date / vendor version |
|---|---|---|---|---|---|---|---|---|
| OC-1 | `/share` | `manual`, `auto`, `disabled` | scalar; "Later configs override earlier ones only for conflicting keys" | global (grim) | permission policy | config.json schema; config.md | schema: `"share": {"type": "string", "enum": ["manual", "auto", "disabled"], "description": "Control whether sessions may be shared manually, automatically, or not at all"}`. The docs page does not mention `share`; `/docs/share` returns 404 | schema fetched 2026-11-05; 1.19.5 |
| OC-2 | `/username` | string | scalar | global | (telemetry) | config.json schema | `"username": {"type": "string", "description": "Username displayed in conversations and used for telemetry identity"}` | as OC-1 |
| OC-3 | `/compaction/reserved` | integer ≥ 0, **deprecated** | scalar | global | behaviour default | config.json schema; config.md | schema: `"reserved": {"minimum": 0, "type": "integer", … "deprecated": true}`; docs: "`reserved` - Token buffer for compaction." | as OC-1 |
| OC-4 | `tui.json` `/theme` | string | not stated for `tui.json` | global `~/.config/opencode/tui.json`; project `tui.json` beside `opencode.json` | behaviour default | tui.json schema; config.md | "Use a dedicated `tui.json` (or `tui.jsonc`) file for TUI-specific settings." / "Use `OPENCODE_TUI_CONFIG` to point to a custom TUI config file." | no date; 1.19.5 |
| OC-5 | `tui.json` `/diff_style` | `auto`, `stacked` | not stated | as OC-4 | behaviour default | tui.json schema; tui.md | `"diff_style": {"type": "string", "enum": ["auto", "stacked"]…}` | as OC-4 |
| OC-6 | `tui.json` `/cursor/style`, `/cursor/blinking` | `block`, `underline`, `line`, `default`; boolean | not stated | as OC-4 | behaviour default | tui.json schema; tui.md | "`style` defaults to `"block"`, can be `"underline"`, `"line"`, or `"default"`; `blinking` defaults to `true`." | as OC-4 |
| OC-7 | `tui.json` `/mouse`, `/attention/{enabled,notifications,sound}`, `/scroll_acceleration/enabled` | boolean each | not stated | as OC-4 | behaviour default | tui.json schema; tui.md | "`mouse` - Enable or disable mouse capture in the TUI (default: `true`)." | as OC-4 |
| OC-8 | file name `config.json` (global) | a third global config file | merged **beneath** `opencode.json` and `opencode.jsonc` (verified) | global | — | config.md (silent); live check | docs name only `~/.config/opencode/opencode.json` and "an `opencode.json` or `opencode.jsonc` file" | no date; 1.19.5 |
| OC-9 (PD-19) | `/permission` vs `/permissions` | V1: `permission` object (action or per-pattern map); V2: `permissions` = array of `{action, resource, effect}` | — | global | permission policy | config.json schema; permissions.md; live check | schema: `"permission": {"$ref": "#/$defs/PermissionConfig", "deprecated": true}`, `"permissions": {"$ref": "#/$defs/Permission.Ruleset", "description": "Ordered tool permission rules applied to agent tool use"}`; permissions.md: "OpenCode uses the `permission` config to decide whether a given action should run automatically, prompt you, or be blocked." | schema fetched 2026-11-05; docs no date; 1.19.5 |

### Live CLI check

All runs: `env -i PATH=… HOME=… TMPDIR=… XDG_CONFIG_HOME=… XDG_DATA_HOME=… XDG_CACHE_HOME=… XDG_STATE_HOME=… OPENCODE_CONFIG_DIR=… OPENCODE_DISABLE_AUTOUPDATE=1 OPENCODE_DISABLE_MODELS_FETCH=1 DO_NOT_TRACK=1 timeout 60 opencode debug config` (native binary `node_modules/opencode-linux-x64/bin/opencode`), value in `$XDG_CONFIG_HOME/opencode/opencode.json` unless noted. A bad value makes it exit 1 with "Configuration is invalid".

- `share` = `manual`, `auto`, `disabled` — pass — exit 0, echoed; control `"sometimes"` exit 1.
- `username` = `"alice"` — pass — exit 0; control `42` exit 1.
- `compaction` = `{auto, prune, reserved: 10000}` — pass — exit 0 (V1 still accepts the deprecated keys); control `prune: "yes"` exit 1.
- **PD-19:** `permission` = `{bash: "ask", edit: {"*.md": "allow", "*": "ask"}}` — pass — exit 0; control `bash: "maybe"` exit 1. `permissions` = `[{action: "bash", resource: "*", effect: "ask"}]` — exit 1: "V2 permissions are not supported by OpenCode V1. Use V1 "permission" rules or run opencode2."
- `snapshots: false` — exit 0, echoed as `"snapshot": false` (V1 maps the V2 name). `update: "notify"` — exit 0, not echoed. An unknown top-level key — exit 0 (V1 does not enforce the schema's `additionalProperties: false`).
- **File name:** `config.json` alone with `share: "auto"` — pass — exit 0, echoed; control `"sometimes"` in `config.json` exit 1 naming `config.json`. `config.json` = `auto` + `opencode.json` = `manual` → `manual`; adding `opencode.jsonc` = `disabled` → `disabled`. Order: `config.json` < `opencode.json` < `opencode.jsonc`.
- `tui.json` with `diff_style: "sideways"` — skip — `debug config` exit 0, so the verb does not read `tui.json`.

### Routing

- Harness: rows (1) · re-check trigger: an OpenCode release that accepts `permissions` (V2 becomes `latest`), or the docs page documenting `share`.
- Settings file: `~/.config/opencode/opencode.json[c]` (Linux, macOS; `$XDG_CONFIG_HOME/opencode/`) · JSONC · `OPENCODE_CONFIG` (a file path). Upstream also reads `config.json` beneath them (OC-8).
- `/share` → row (`Enum[manual, disabled]`, Scalar, global). `auto` is left out on purpose: it shares every session automatically, a data-egress loosening of the kind the telemetry exclusion covers. If the owner rejects a narrowed enum, off for that reason.
- `/username` → off: "used for telemetry identity"; telemetry is scoped out, and it is not a behaviour default.
- `/compaction/reserved` → off: deprecated in the schema, and an integer (no number type).
- `tui.json` keys (OC-4 to OC-7) → off: a second settings file per harness (`tui.json`, relocated by `OPENCODE_TUI_CONFIG`), and `settings_config_path` returns one file per harness. It needs an ADR decision; the values above are verified from the vendor schema and docs if it is taken.
- `config.json` file name → off as a row; recorded as a defect finding below.

---

## Qoder

Vendor version checked: `@qoder-ai/qodercli` 1.1.83 (npm). Pages:
[settings reference](https://docs.qoder.com/cli/settings-reference.md),
[settings](https://docs.qoder.com/cli/settings.md),
[permissions](https://docs.qoder.com/cli/permissions.md); no date shown, fetched 2026-11-05.

### Claims

| # | Key (JSON pointer) | Value type and values | Merge | Scope | Category | Source URL | Verbatim quote | Page date / vendor version |
|---|---|---|---|---|---|---|---|---|
| QO-1 | `/model/reasoningEffort` | `disabled`, `off`, `none`, `low`, `medium`, `high`, `xhigh`, `max` | scalar: "Single values (strings, numbers, booleans): Directly overridden by higher precedence." | user, project, local | behaviour default | settings-reference.md; settings.md | reference: "`model.reasoningEffort` \| enum \| None \| No \| Reasoning Effort: `disabled`/`off`/`none`/`low`/`medium`/`high`/`xhigh`/`max`." settings.md (non-exhaustive): "Reasoning Effort Level: `low` / `medium` / `high`, etc." The package's bundled settings schema (`bundle/qodercli.js`, 1.1.83) lists the same eight `options` values | no date; 1.1.83 |
| QO-2 | array merge between user and project files (`/permissions/{allow,ask,deny}`) | — | **unresolved** | — | permission policy | settings.md; permissions.md | settings.md: "**Arrays**: Some configuration items (such as disable lists and exclude lists) use "union merge" to combine and deduplicate values across levels; other arrays are overridden by default." permissions.md: "Rules merge from multiple sources, from lowest to highest priority" … "Higher-priority sources override lower ones." | no date; 1.1.83 |

QO-2 note: neither page names `permissions.allow`/`ask`/`deny` as union-merged or as overridden.
The bundled schema marks some arrays `mergeStrategy:"union"`, but its `permissions` object defines
only `additionalDirectories` and `trustDirectories`, so it does not settle the rule arrays either.

### Live CLI check

- `env -i PATH=… HOME=… TMPDIR=… XDG_CONFIG_HOME=… QODER_CONFIG_DIR=… DO_NOT_TRACK=1 timeout 60 node bundle/qodercli.js --version` / `--help` — pass — exit 0, 1.1.83; node from the repo toolchain linked into the scratch bin.
- Settings check — not run: `--help` lists no `config`, `doctor` or validate verb (commands: hooks, update, rollback, plugins, mcp, commit, agents, skills, feedback, security, login, status, remote-control, sites, wiki), and an unknown word is taken as a prompt, which would start a session.

### Routing

- Harness: rows (1) · re-check trigger: the settings or permissions page states the merge for `permissions.*`, or a Qoder CLI release adds a settings-validating verb.
- Settings file: `~/.qoder/settings.json` · JSON (grim treats it as JSONC) · `QODER_CONFIG_DIR`.
- `/model/reasoningEffort` → row (`Enum[disabled, off, none, low, medium, high, xhigh, max]`, Scalar, global).
- Qoder array merge, project vs user → off as a resolution: still unresolved; the shipped `/permissions/*` rows stay `on-limited`.

---

## Claude settings-key check (S-030, closing-review fix pass)

The settings plan's S-030 asks a deep pass on Claude to report the settings keys upstream against
`settings_fields()`. Run on 2026-11-05 as a web read only: [settings reference](https://code.claude.com/docs/en/settings-reference.md)
and [sandboxing](https://code.claude.com/docs/en/sandboxing.md) fetched raw with `curl`, no page
date; npm `@anthropic-ai/claude-code` latest is 2.1.318, the version of every live run above. No
binary ran. Not a full `/upstream-refresh` deep pass, so the ledger's `Last check` stays.

**Key list.** Each of the 23 rows (`settings.md#fields`, claude) has its own heading on the
reference page, and its stated type matches grim's: `model`, `outputStyle`, `language`,
`attribution.commit`, `attribution.pr` strings; `spinnerTipsEnabled`, `alwaysThinkingEnabled`,
`includeGitInstructions`, `sandbox.enabled`, `attribution.sessionUrl` Booleans;
`permissions.allow`, `deny`, `ask` "array of permission rule strings"; the four
`sandbox.filesystem.*` "array of path strings"; `extraKnownMarketplaces` "object mapping a
marketplace name to an object with a `source` object and an optional `autoUpdate` Boolean"; and
the enums `effortLevel` (`low`, `medium`, `high`, `xhigh`), `theme` (the seven grim takes, plus
`custom:` forms), `editorMode` (`normal`, `vim`), `tui` (`default`, `fullscreen`) and
`permissions.defaultMode`. That last one now lists `manual` itself: "`"manual"`: an alias for
`"default"`", which settles the withdrawn PX-4 finding from the docs side too. No row was dropped or
retyped.

**Limits the page states that grim does not name** (findings 8 to 11):

- `effortLevel` in the user file, grim's global target: "In your user settings file,
  `~/.claude/settings.json`, this key is the older form `/effort` wrote before it saved levels per
  model, and it keeps applying where it applied before, on Opus 5, Fable 5.1, and earlier models.
  Opus 5.5 and models released after it ignore it and start at their own default until you save a
  level for them, which `/effort` writes under [`modelSettings`](#modelsettings). In project, local,
  and managed settings, and with `--settings`, this key applies to every model."
- `sandbox.enabled`: "**Scope**: `Any file`, with limits on project and local settings". The
  sandboxing page's table for an admin-required sandbox: "`enabled`, `failIfUnavailable` |
  `false`, when the developer's `~/.claude/settings.json` sets `true`". The same table limits the
  sandbox effect of `Edit(...)` and `WebFetch(domain:...)` allow rules from a repository file; the
  tools still follow the rules.
- `permissions.defaultMode`: "For conversations the VS Code extension starts, Claude Code reads
  only user, managed, and `--settings` values." A project value grim writes is unread there.
- `extraKnownMarketplaces`: "Claude Code honors entries in a repository's `.claude/settings.json`
  or `.claude/settings.local.json` only after you accept the workspace trust dialog for that
  folder; in a folder you haven't trusted, including a `-p` run there, it ignores them without a
  message."

## Findings for the orchestrator (not rows)

1. **PD-19 verdict: drift confirmed, as a V1/V2 split.** The hosted schema (https://opencode.ai/config.json, fetched 2026-11-05, no date) describes both generations: it marks `permission` deprecated and adds the V2 `permissions` array. The docs pages (no date) show only the V1 object form. The current release, 1.19.5 (npm `latest`, 2026-10-28), accepts `permission` and **rejects** `permissions` ("V2 permissions are not supported by OpenCode V1 … run opencode2"). The first-slice `/permission/<tool>` rows are correct for today's V1. The same schema also marks four more first-slice rows deprecated: `/autoupdate` (V2 `update`), `/snapshot` (V2 `snapshots`), `/small_model` and `/compaction/prune`. That is 5 of 9 OpenCode rows on a V2 deprecation path; V1 accepts all five (verified). Re-check trigger: V2 becomes `latest`. No change to a first-slice row is proposed.
2. **OpenCode `config.json` gap is real (defect class: a user value silently outranked).** 1.19.5 reads `~/.config/opencode/config.json` and merges it beneath `opencode.json` and `opencode.jsonc` (verified). `src/install/opencode_config.rs:123-133` resolves only `opencode.jsonc`/`opencode.json`. A grim write there outranks a user's value in `config.json` that grim never read, against ADR decision driver 4. Fix pass or issue: read `config.json` as a lower layer for conflict detection, or decline when it sets the pointer.
3. **OpenCode `PermissionConfig` names a `list` tool** that grim's `/permission/<tool>` `map_keys` lacks. The docs page's list matches grim's; the schema adds `list` and allows other names. Out of scope here; noted for the next OpenCode refresh.
4. **Claude `attribution: false`** (v2.1.281+) is a non-object at the container the three proposed sub-key rows recurse into. The splice must refuse there, never replace `false`.
5. **Codex granular approval:** Codex silently ignores a misspelled inner key (verified), so even a future typed table must close the inner key set itself.
6. **Copilot reads `.claude/settings.json`** for "the shared cross-tool subset of repository settings (such as `companyAnnouncements`, `disableAllHooks`, `enabledPlugins`, `extraKnownMarketplaces`, and `hooks`)". Already known (field list § 6); a Claude project-scope write of `/extraKnownMarketplaces/<name>` reaches Copilot too.
7. **Cursor changelog v2026.07.13** names a cli-config key the reference page lacks: "set `autoAcceptWebSearch` in `cli-config.json`". Candidate for a later Cursor refresh, not verified to a row's standard here. Routed on 2026-11-05: off (CU-4).
8. **Claude `/effortLevel` at global scope** reaches only Opus 5, Fable 5.1 and earlier models: Opus 5.5 and later ignore the key in the user file (S-030). At project scope (`settings.local.json`) it applies to every model. A named limit for `settings.md`, or an `on-limited` mark, is the orchestrator's call.
9. **Claude `/sandbox/enabled` at project scope** has a stated limit: under an admin-required sandbox a repository `false` is ignored when the user file sets `true` (S-030). The row is not `on-limited` today.
10. **Claude project `/permissions/defaultMode`** is unread in conversations the VS Code extension starts (S-030).
11. **Claude project `/extraKnownMarketplaces/<name>`** is honoured only after the workspace trust dialog, and ignored silently before it (S-030).

## Verifier sign-off

none required

## Issue drafts

none (findings for the orchestrator go in the report)

## Friction

- The Copilot docs page renders at about 240,000 characters; the raw markdown from `github/docs` is 100,930 bytes and fits one read. Use the raw file next time.
- OpenCode `/docs/share.md` and `/docs/share/` return 404; `share` is documented only in the schema.
- `opencode.ai/config.json` mixes V1 and V2 keys with no version marker. Only the binary's error message told the two apart; the deprecation flags alone would have suggested a V1 change.
- `claude doctor` checks `tui` and `attribution` types but not `theme` or `editorMode` values, so two controls passed and those checks are recorded `skip`.
- Qoder's bundle is obfuscated; a first regex search over the 33 MB file timed out and was redone with a fixed-string search.
- The scoping artifact's "Copilot tool rules … none exist as settings keys" is confirmed and sharpened: they exist as **managed** keys, refused at user level.
- Dropped claim: an inferred Qoder per-source rule evaluation from bundle code. Not a primary statement, so not used.

## Skill changes

none

## Sources

All fetched 2026-11-05 with `curl`; none shows a page date.

- Claude: https://code.claude.com/docs/en/settings.md, https://code.claude.com/docs/en/settings-reference.md, https://code.claude.com/docs/en/sandboxing.md
- Codex: https://learn.chatgpt.com/docs/config-file/config-reference.md, https://learn.chatgpt.com/docs/config-schema.json, https://learn.chatgpt.com/docs/agent-configuration/rules.md
- Copilot: https://raw.githubusercontent.com/github/docs/main/content/copilot/reference/copilot-cli-reference/cli-config-dir-reference.md (file last changed 2026-11-03); added 2026-11-05 for CP-56: https://raw.githubusercontent.com/github/docs/main/content/copilot/how-tos/copilot-cli/set-up-copilot-cli/configure-copilot-cli.md (file last changed 2026-10-28)
- Cursor: https://cursor.com/docs/cli/reference/configuration.md, https://cursor.com/docs/agent/security/run-modes.md, https://cursor.com/docs/cli/reference/parameters.md, https://cursor.com/docs/cli/changelog.md, https://cursor.com/docs/cli/installation.md; added 2026-11-05 for CU-4: https://cursor.com/docs/cli/reference/permissions.md
- OpenCode: https://opencode.ai/config.json, https://opencode.ai/tui.json, https://opencode.ai/docs/config.md, https://opencode.ai/docs/permissions.md, https://opencode.ai/docs/tui.md
- Qoder: https://docs.qoder.com/cli/settings-reference.md, https://docs.qoder.com/cli/settings.md, https://docs.qoder.com/cli/permissions.md

Search terms: `<harness> settings reference`, `<harness> config schema json`,
`copilot cli config-dir reference raw github/docs`, `opencode V2 permissions opencode2`.
