# Junie (JetBrains) — hook / lifecycle-event research

Research date: 2026-08-14. All fetches performed today unless noted.

## 0. Product-surface disambiguation (read this first)

JetBrains ships **two distinct Junie surfaces** that share the `.junie/` directory
namespace but are otherwise separate products with separate docs sites:

1. **Junie IDE plugin** — bundled/installable inside IntelliJ-platform IDEs. Docs at
   `www.jetbrains.com/help/junie/` and `www.jetbrains.com/help/idea/junie.html`.
   This is almost certainly what the brief's "known grim-relevant paths" describes.
2. **Junie CLI** — a standalone, LLM-agnostic terminal agent ("Run Junie directly from
   your terminal, in any IDE, or inside your CI/CD pipelines"), announced ~mid-2025,
   promoted from EAP to **Beta** per a JetBrains blog post dated March 2026. Docs at
   the *separate* site `junie.jetbrains.com/docs/` (note: `www.jetbrains.com/help/junie/`
   301-redirects here now, i.e. JetBrains has folded the help redirect into the CLI docs
   site as of this fetch).

**The entire hook mechanism lives in Junie CLI, not the IDE plugin.** I found zero
mention of hooks, lifecycle events, or automations in the IDE plugin doc
(`junie-ide-plugin.html`) — it only covers guidelines/AGENTS.md and MCP config. The CLI
and the IDE plugin share the *directory* (`.junie/` at project root, `~/.junie/` global)
but the CLI layers additional files into it: `config.json`, `allowlist.json`, and a
`trust/` marker directory, none of which the IDE plugin reads.

Path correction versus the brief's assumption: `.junie/guidelines.md` is now the
**deprecated legacy** location; current docs point to **`.junie/AGENTS.md`** (backward
compatible with old `guidelines.md`). MCP project config is `.junie/mcp/mcp.json`
(confirmed). I found no `.junie/rules/*.md` convention documented anywhere — it may not
exist, or may be IDE-version-specific; treat the brief's "rules/*.md" as unconfirmed.

---

## 1. Existence & name

**Yes — Junie CLI has a real, documented hook system called "Hooks."**

- Vendor's own framing (from `junie-cli-hooks.html`): "Hooks let you run shell commands
  automatically at well-defined points in a Junie CLI session."
- **Status: Early Access Program (EAP)**, explicitly gated separately from the rest of
  Junie CLI (which itself is Beta, not GA). Exact quote from the hooks page: **"This
  feature is currently in the Early Access Program. To try it, install the Early Access
  version of Junie CLI."** This is a feature-level EAP flag layered on top of a
  product-level Beta — i.e., even users on stable/Beta Junie CLI do not get hooks;
  they must opt into the separate EAP build.
- The hooks doc page itself carries a date stamp of **12 August 2026** — two days before
  this research — so this is freshly published/documented and should be treated as
  liable to change without a formal deprecation notice.
- No stability tier lower than EAP (no "experimental" or "alpha" label used); no
  deprecation notice found.

## 2. Config location(s)

- **Global/user scope:** `~/.junie/config.json` (also documented as `<Junie Home>`,
  default `~/.junie`).
- **Project scope:** `<project-root>/.junie/config.json` — **but hooks specifically are
  excluded from this file by default** (see §9 below — this is the single most important
  gotcha for third-party installers).
- **Env vars:**
  - `JUNIE_CONFIG_LOCATION` — additional config.json path(s); **repeatable** (can be
    passed multiple times, e.g. `--config-location A --config-location B`, or the env-var
    equivalent).
  - `JUNIE_CONFIG_DEFAULT_LOCATIONS` — boolean, enable/disable the default locations
    above (defaults to `true`).
  - Equivalent CLI flag: `--config-location` (repeatable).
- **Merge behavior across multiple config files (verbatim):** "When multiple
  configuration files define hook entries for the same event, all entries from all files
  are concatenated. Higher-priority files do not override lower-priority files; their
  entries are appended." — i.e., for the `hooks` key specifically, it's a **union/append,
  not override**. (Note: this is specific to `hooks`; other config.json scalar keys, per
  the general precedence list below, do override.)
- **General config.json precedence (highest → lowest), for non-hook keys:**
  1. Command-line flags
  2. User settings `~/.junie/settings.json`
  3. Project config (only when the project directory is "trusted", see §9)
  4. User config `~/.junie/config.json`
- No separate hooks-only directory convention (no `<root>/hooks/*.json` auto-discovery)
  was documented — hooks live inline under the `"hooks"` key of `config.json`.
- Directory-convention analogs exist for *other* artifact kinds Junie CLI supports, e.g.
  `command-locations`, `skill-locations`, `agent-locations`, `model-locations` (each with
  a paired `*-default-locations` boolean) — but hooks are not one of them; hooks are
  config-file-only, not directory-scanned.

## 3. Config schema — verbatim

**Named map of event name → array of entries** (NOT a flat array):

```json
{
  "hooks": {
    "SessionStart": [...],
    "UserPromptSubmit": [...],
    "PreToolUse": [...]
  }
}
```

Each **entry** in an event's array:

```json
{
  "matcher": "startup",
  "hooks": [
    {
      "type": "command",
      "command": "script.sh",
      "timeout": 30,
      "blockOnError": false,
      "async": false
    }
  ]
}
```

- `matcher` (optional, regex): matches against an event-specific value (e.g. the
  `source` for `SessionStart`, the tool name for `PreToolUse`/`PermissionRequest`).
  Omitted = matches all. Pipe-alternation supported: `"startup|resume"`.
- `hooks` (required): **array** of hook-command objects — i.e. one entry can fan out to
  multiple commands.
- Per-command fields: `type` (only `"command"` is currently supported — verbatim: "Only
  `type: "command"` hooks are currently supported" — no JS/TS module type, no HTTP type),
  `command` (a shell command **string**, not an argv array), `timeout` (seconds,
  overrides the per-event default), `blockOnError` (boolean, **`Stop`-only** — see §7),
  `async` (boolean, see §5/§8).
- **No id/name/description field on any hook entry or command** is documented anywhere.
  This matters directly for the brief's "stable identity" question: **there is currently
  no vendor-native field a third-party installer could use as an idempotent handle** to
  own/update/remove a single hook entry. An installer would have to invent its own
  convention (e.g. a marker comment inside `command`, or owning a dedicated
  `--config-location` file it fully manages) rather than rely on schema-native identity.
- Full real multi-event example, copied verbatim from the docs:

```json
{
  "hooks": {
    "SessionStart": [
      {
        "matcher": "startup",
        "hooks": [
          { "type": "command", "command": "aws sso login --profile dev", "timeout": 30 }
        ]
      }
    ],
    "UserPromptSubmit": [
      {
        "hooks": [
          { "type": "command", "command": "~/.junie/scripts/check-prompt.sh" }
        ]
      }
    ],
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          { "type": "command", "command": "~/.junie/hooks/check-bash-command.sh" }
        ]
      }
    ],
    "SessionEnd": [
      {
        "matcher": "prompt_input_exit|logout",
        "hooks": [
          { "type": "command", "command": "~/.junie/scripts/flush-session-logs.sh" }
        ]
      }
    ],
    "PermissionRequest": [
      {
        "matcher": "Bash",
        "hooks": [
          { "type": "command", "command": "~/.junie/scripts/check-bash-permission.sh" }
        ]
      }
    ]
  }
}
```

Note the striking structural resemblance to Claude Code's hook schema: same
`{"hooks": {"<Event>": [{"matcher", "hooks": [{"type": "command", "command", "timeout"}]}]}}`
shape, and — per §7 — the same response vocabulary (`decision`, `continue`,
`systemMessage`, `hookSpecificOutput`, `additionalContext`, `stopReason`). This looks like
a deliberate design borrowing, which is good news for a portable-schema effort: a
Claude-Code-shaped abstraction maps onto Junie CLI hooks with very little translation.

## 4. Event catalogue — verbatim, with firing point

Seven events total. Exact one-line descriptions from the docs:

| Event | Verbatim firing description | Category |
|---|---|---|
| `SessionStart` | "Junie fires `SessionStart` once per session with one of the following sources" (sources: `startup\|resume\|clear\|compact`) | session lifecycle |
| `UserPromptSubmit` | "Junie fires `UserPromptSubmit` every time the user submits a prompt in the interactive TUI, before the prompt is sent to the model." | prompt submit |
| `PreToolUse` | "Junie fires `PreToolUse` before each tool call, after the action request is parsed but before the tool executes." | pre-tool-use |
| `Stop` | "Junie fires `Stop` synchronously right before the agent transitions a task to a successful submission" | stop/finish |
| `StopFailure` | "Junie fires `StopFailure` once per agent turn when the LLM/API call backing that turn ends in a documented failure" (rate_limit, authentication_failed, billing_error, invalid_request, server_error, max_output_tokens, unknown, model_refused, country_forbidden) | error |
| `PermissionRequest` | "Junie fires `PermissionRequest` whenever it is about to show a permission dialog asking the user to approve a sensitive action." | notification / approval |
| `SessionEnd` | "Junie fires `SessionEnd` whenever a session terminates, with one of the following reasons" (reasons: `prompt_input_exit\|other\|logout`) | session lifecycle |

Notable absences versus, e.g., Claude Code: **no post-tool-use event**, **no explicit
file-edit-specific event** (file edits are covered generically via `PreToolUse` matched
against the `Edit`/`Write` tool names), **no compaction-specific event** (compaction is
folded into `SessionStart`'s `source: "compact"` value), **no subagent event** documented.

`PermissionRequest` is the important cross-link: it fires "whenever [Junie] is about to
show a permission dialog" — i.e. it's the programmable escape hatch for the same
decision surface as Brave Mode / the Action Allowlist (§ Disambiguation below). A hook
here can auto-answer what would otherwise be a human approval prompt.

## 5. Invocation

- **Shell command string**, not an argv array, not a JS/TS module, not an HTTP endpoint.
- Verbatim: **"Commands execute via `sh -c` on macOS/Linux and `cmd /c` on Windows."**
- **Blocking by default.** Verbatim: "By default, a hook blocks the triggering action
  until it completes — the prompt waits for `UserPromptSubmit`, the tool waits for
  `PreToolUse`, and so on."
- **`async: true`** opt-out, verbatim: "For long-running tasks (test suites, deployments,
  external API calls) set `"async": true` on the hook command to run it in the background
  while the agent keeps working." Effects of async mode: the triggering action proceeds
  immediately; `decision`/`permissionDecision`/`continue` are logged then ignored;
  `systemMessage` is shown only as a TUI notification; `additionalContext` is queued for
  the *next* user submission; timeout/failure handling still applies. **Async is not a
  legal option for `SessionStart`, `SessionEnd`, `StopFailure`** — verbatim: "SessionStart,
  SessionEnd, and StopFailure already run in the background at the executor level — Junie
  never waits for them" (i.e. they're unconditionally fire-and-forget already).
- **Working directory:** NOT DOCUMENTED. No page states what CWD the hook process
  inherits (project root vs. wherever `junie` was invoked from).
- **`$PATH` handling:** NOT DOCUMENTED. No statement about environment variable
  inheritance/sanitization for the child process.
- **Timeouts** (defaults, overridable per-command via `timeout` in seconds):
  - `SessionStart`, `UserPromptSubmit`, `PermissionRequest`: 10s
  - `Stop`: 600s
  - `StopFailure`: 60s (capped 60s total across all StopFailure hooks)
  - `SessionEnd`: 2s (capped 10s total)
- **Concurrency/ordering:** Verbatim: "Hooks within a single entry run sequentially.
  Parallel execution is not supported." The docs do **not** state whether multiple
  *entries* matching the same event (e.g. two separate `PreToolUse` blocks both matching
  `Bash`) run sequentially or concurrently relative to each other, nor whether
  declaration order is respected across entries — NOT DOCUMENTED beyond the
  within-entry guarantee.
- **Missing binary / exec failure:** NOT DOCUMENTED as a distinct case from "non-zero
  exit" — the docs only describe exit-code handling (§7), not what happens if the
  command can't be spawned at all (e.g. command not found).

## 6. Input payload — verbatim (JSON on stdin, one object per event)

```json
{"hook_event_name":"SessionStart","source":"startup|resume|clear|compact"}
{"hook_event_name":"UserPromptSubmit","prompt":"…"}
{"hook_event_name":"PreToolUse","tool_name":"Bash|Write|Read|Edit|Glob|Grep","tool_input":{...}}
{"hook_event_name":"Stop","stop_hook_active":false,"last_assistant_message":"…"}
{"hook_event_name":"PermissionRequest","tool_name":"Bash|Edit|Read|MCP_tool_name","tool_input":{}}
{"hook_event_name":"StopFailure","error":"rate_limit|authentication_failed|billing_error|invalid_request|server_error|max_output_tokens|unknown|model_refused|country_forbidden","error_details":"…"}
{"hook_event_name":"SessionEnd","reason":"prompt_input_exit|other|logout"}
```

All payload delivery is **stdin JSON only** — no env-var-based payload delivery and no
`{{template}}` interpolation into the `command` string was documented anywhere (contrast
with clients that support `$TOOL_NAME`-style interpolation — Junie does not appear to).

## 7. Output / response contract — verbatim

**Exit codes:**
- `0` = success, hook completes normally.
- `2` = "block decision" (for applicable hook types).
- Any other non-zero = "Warning logged; action proceeds (or falls back to user dialog)."
- `StopFailure` is the one documented exception: **"Output and exit code from the hook
  process are ignored"** — it's observability-only, no response contract at all.

**stdout parsing:** attempted as JSON; for `PreToolUse` specifically, verbatim: **"If the
hook output is not valid JSON, the raw stdout is treated as `additionalContext`."** (Not
stated for other events — NOT DOCUMENTED whether the same non-JSON→additionalContext
fallback applies to `Stop`/`PermissionRequest`/etc.)

**stderr:** verbatim — **"Standard output and standard error are captured and logged at
debug level."** (i.e. not surfaced to the user or model by default except through the
explicit response fields below; only visible via debug logs.)

**`PreToolUse` response schema:**
```json
{
  "decision": "allow|ask|block|deny",
  "reason": "human-readable message",
  "updatedInput": {...},
  "additionalContext": "text"
}
```
Decision semantics (verbatim/paraphrased from the docs): `allow` (or the field omitted)
— "the tool runs with its original (or updated) input"; `ask` — "Junie pauses and asks
the user to confirm before the tool runs" (defers to the human approval dialog); `block`
or `deny` — "the tool is not executed; the model receives an error message" (the docs
treat `block` and `deny` as equivalent outcomes — no distinction found between them).
`updatedInput` lets the hook rewrite the tool call's input before execution.

**`Stop` response schema:**
```json
{
  "decision": "block",
  "reason": "…",
  "continue": false,
  "stopReason": "…",
  "hookSpecificOutput": {"additionalContext": "…"},
  "systemMessage": "…"
}
```
`blockOnError` (Stop-only field, on the *command*, not the response): verbatim —
"`blockOnError`: `Stop` hooks only. When `true`, any non-zero exit code (other than the
already-blocking `2`) is promoted to a block-with-retry, with the command's stdout+stderr
fed back to the agent as the block reason. Defaults to `false`. Ignored for other events."

**`PermissionRequest` response schema:**
```json
{"decision": "allow|block|deny"}
```
Exit 0 with no `decision` field = auto-approve (`PermissionRequest` only).

**`SessionStart`, `UserPromptSubmit`, `SessionEnd`:** NOT DOCUMENTED — the docs never
spell out a response JSON schema for these three events (unlike Claude Code, which
documents `additionalContext` for `SessionStart`/`UserPromptSubmit` explicitly). Given the
general architecture it's plausible they accept at least `additionalContext`, but I found
no page stating this — treat as unconfirmed, not assumed.

**Visibility to user vs. model:** `additionalContext` → model (added to what the agent
sees). `systemMessage` → user (TUI notification; in async mode, notification-only).
`reason` → appears to be the error message returned to the model on block/deny. Direct
stdout/stderr → neither user nor model by default, only debug logs.

## 8. Reliability & limits

- Timeouts: see §5 table. Overridable per-command via `timeout`.
- Non-zero exit (not 2): "Warning logged; action proceeds (or falls back to user
  dialog)" — i.e. **fails open**, not closed, except for the explicit `2` block code and
  `Stop`'s `blockOnError` promotion path.
- Malformed (non-JSON) output: for `PreToolUse`, silently reinterpreted as
  `additionalContext` text rather than treated as an error — NOT DOCUMENTED for other
  events.
- Missing binary: NOT DOCUMENTED as distinct from generic non-zero-exit handling.
- Parallelism: none within an entry (sequential); cross-entry/cross-event concurrency
  NOT DOCUMENTED.
- Blocking vs fire-and-forget: blocking by default per event (with per-event timeout
  ceilings), opt-out via `async: true`, except `SessionStart`/`SessionEnd`/`StopFailure`
  which are unconditionally fire-and-forget ("already run in the background at the
  executor level").

## 9. Security posture

This is where Junie CLI is most explicit, and it's the single biggest finding for
third-party installability:

- **Project-scoped hook config is ignored by default.** Verbatim: **"Project-local hooks
  from `<project-root>/.junie/config.json` are ignored by default for safety. Project
  configuration is repository-controlled, so Junie will not run shell commands from it
  automatically."** The doc's prescribed alternative: **"Use `~/.junie/config.json` for
  personal hooks, or pass a hook config file explicitly with `--config-location`."**
- This sits inside a broader **project-trust system** (`<Junie Home>/trust`, default
  `~/.junie/trust`): each project directory (or a parent-directory scope) gets an
  authenticated trust marker, stored via OS keychain (macOS Keychain / Windows Credential
  Manager / Linux Secret Service) or, as a fallback, an owner-only key file in the trust
  directory itself — explicitly so "your decision is remembered on headless machines and
  in containers."
- Verbatim on what an untrusted project can and cannot do: **"An unknown project
  continues in restricted mode with writable temporary project Junie storage outside the
  repository, so ordinary project reading and editing still work without loading
  repository-controlled MCP servers, hooks, agents, skills, or guidelines."** — hooks are
  explicitly named in the same bucket as MCP servers and skills as things withheld from
  untrusted projects.
- Interactive-mode trust prompts offer three choices (paraphrased): keep untrusted, trust
  only the exact project directory, or trust the whole parent directory.
- **Non-interactive modes (headless/CI, Gateway, ACP) currently do NOT enforce the trust
  gate** — verbatim: "Interactive UI launches always enforce project trust,"
  but "Trust-marker enforcement for these non-UI modes is controlled by a build rollout
  toggle and is currently disabled, so they retain their previous trusted behavior," and
  "Gateway and ACP modes retain their existing trusted behavior while the non-interactive
  rollout toggle is disabled." In plain terms: headless/CI runs currently trust the
  project by default (rollout of stricter enforcement is in progress but off) — a
  meaningful, dated caveat (fetched 2026-08-14) since this is described as a toggle
  JetBrains is actively rolling out, i.e. expect this to tighten later.
- No explicit "hooks are arbitrary code execution, be careful" warning sentence was found
  verbatim on the hooks page itself — the warning is implicit in the project-trust
  design and the "ignored by default for safety" line above, rather than a standalone
  disclaimer paragraph.
- Separately (**not a hook mechanism — disambiguation**): **Brave Mode** (off / auto / on,
  toggle via `/brave` or Ctrl+B) and the **Action Allowlist** (`~/.junie/allowlist.json`)
  govern whether Junie pauses for human approval before `fileEditing` (outside-project or
  build-script edits), `executables` (terminal commands, tests, builds), `mcpTools`, and
  `readOutsideProject` actions. Allowlist rules use `prefix` (literal string match) or
  `pattern` (glob with `*`, `**`, `?`, `[abc]`, `[!abc]`) matching, each mapped to `allow`
  or `ask`, first-match-wins. This is a **policy/approval gate**, not an
  event-driven-user-code hook — but it composes with hooks via the `PermissionRequest`
  event (§4), which fires exactly when this system would otherwise show its dialog.

## 10. Third-party installability

- **File-edit installable, in principle** — `config.json` is a plain JSON file an
  external tool can read/write.
- **But the default project-scope path is a dead end.** Because
  `<project-root>/.junie/config.json`'s `hooks` key is ignored unless the end user
  explicitly launches with `--config-location` pointing at it (or copies content into
  `~/.junie/config.json`), a package manager that materializes hooks the same way it
  materializes rules/skills/MCP config into the project's `.junie/` tree **would silently
  no-op** for hooks specifically, even though every other artifact kind in the same
  directory works normally. This is the standout gotcha the brief asked to look for.
  Realistic options for an installer: (a) write to the **global** `~/.junie/config.json`
  (works out of the box, but is not project-scoped — pollutes/affects all projects), or
  (b) write a dedicated file and instruct/automate the user to pass
  `--config-location <path>` (or set `JUNIE_CONFIG_LOCATION`) — extra setup step, not
  silent, but scriptable.
- **No stable per-entry identity field** (§3) — an installer must own a whole
  `--config-location` file (or a well-known key range within one) to update/remove its
  own entries idempotently, rather than mutating a single entry in place safely.
  Concatenation-not-override merge semantics (§2) make this more forgiving (no clobber
  risk from co-existing files) but also mean a naive re-install without dedup would
  accumulate duplicate entries if it also touched `~/.junie/config.json` directly instead
  of owning a dedicated file.
- **Restart / snapshot gotcha:** NOT DOCUMENTED explicitly ("config is snapshotted at
  startup" phrasing was not found), but the trust-marker system is described as taking
  effect "for the next process launch" when revoked, which implies config in general
  (trust at least) is read at process start, not hot-reloaded — treat a restart as
  necessary until documented otherwise for hooks specifically.
- **Not UI-only, not cloud-only** — it is a real local file + local shell-exec mechanism,
  CLI-native. It is, however, **feature-gated behind installing a separate EAP build** of
  Junie CLI (§1), so "third-party installable" today implicitly also requires the end
  user to be on that EAP build for the hooks to do anything at all.
- **Headless/CI note:** `junie-headless.html` confirms Junie CLI is designed to run in
  CI/CD ("Junie CLI in headless mode without interactive UI in CI/CD environments and
  build pipelines," invoked like `junie --auth="$JUNIE_API_KEY" "<prompt>"`), and per §9
  those non-interactive runs currently skip the trust gate entirely (rollout toggle off),
  meaning a project's own `.junie/config.json` hooks would in fact be trusted/loaded in
  today's CI runs even though they're withheld interactively — an inconsistency worth
  flagging, not something I'm fully certain is intentional long-term behavior.

## 11. Trampoline viability

**Good candidate for a generic trampoline**, better than most clients surveyed, because:

- Invocation is a **plain shell command string** (`sh -c` / `cmd /c`) — a trampoline like
  `grim hook run --client junie --event <E>` slots in directly as the `command` value.
- Payload is **stdin JSON**, response is **stdout JSON** — no in-process JS/TS module
  requirement, no HTTP requirement, unlike clients whose hook handler must be a language-
  native function.
- The event/response vocabulary (`decision`, `continue`, `systemMessage`,
  `hookSpecificOutput`, `additionalContext`, `matcher`, `type: "command"`) closely mirrors
  Claude Code's, which means a portable schema targeting Claude Code's shape would need
  only small per-field renames/omissions to also target Junie, not a structurally
  different adapter.

**Named blockers:**

1. **EAP gate** — hooks don't exist in any Junie CLI build a typical user already has
   installed; the trampoline is dead code until the user opts into EAP.
2. **Project-scope hooks ignored by default** (§9/§10) — the most severe blocker: a
   naive "materialize into `.junie/config.json`" installer produces a file that does
   nothing until the user adds an explicit CLI flag or env var, or the installer instead
   targets the global file (losing project scoping).
3. **No stable per-entry id** — the trampoline's *registration* (not its invocation) has
   no schema-native anchor to update/remove idempotently; grim would need to own a whole
   dedicated `--config-location` file per its managed hook set to make this safe.
4. **Missing docs for non-JSON stdout on events other than `PreToolUse`**, missing docs
   for `SessionStart`/`UserPromptSubmit`/`SessionEnd` response schemas, missing docs for
   CWD/`$PATH` — a trampoline implementation would have to test-and-verify these empirically
   against the EAP build rather than trust the docs, since they're silent.
5. **IDE-plugin users get nothing** — if grim's Junie "client" identity is meant to also
   cover the IDE-plugin surface (which is what the brief's known paths suggest), hooks
   are simply unavailable there; the trampoline only ever applies to the CLI surface.

---

## Sources

| URL | What it establishes | Fetched |
|---|---|---|
| https://junie.jetbrains.com/docs/junie-cli-hooks.html | Full hooks reference: events, config schema, example, timeouts, exit codes, response schemas, matcher syntax, async behavior, EAP status, security note, stderr/stdout handling, decision semantics | 2026-08-14 |
| https://junie.jetbrains.com/docs/junie-cli-configuration.html | config.json schema, file locations, env vars (`JUNIE_CONFIG_LOCATION`, `JUNIE_CONFIG_DEFAULT_LOCATIONS`), precedence order, hooks cross-reference, project-hooks-ignored caveat | 2026-08-14 |
| https://junie.jetbrains.com/docs/action-allowlist.html | Action Allowlist UI description, action types (Terminal/RunTest/Build/Preview/MCP/Read-outside-project/Write-outside-project/Edit-build-scripts), regex examples; confirms this is approval-only, not a hook mechanism | 2026-08-14 |
| https://junie.jetbrains.com/docs/action-allowlist-junie-cli.html | `~/.junie/allowlist.json` location, action categories (fileEditing/executables/mcpTools/readOutsideProject), `prefix`/`pattern` matching, allow/ask values, first-match-wins | 2026-08-14 |
| https://junie.jetbrains.com/docs/junie-cli.html | Junie CLI product identity/description, mentions custom slash commands, MCP, skills, subagents; no stability tier stated on this page | 2026-08-14 |
| https://junie.jetbrains.com/docs/junie-cli-eap.html | EAP program description ("pre-release versions... before generally available"); no hooks-specific EAP framing found here (framing is on the hooks page itself) | 2026-08-14 |
| https://junie.jetbrains.com/docs/junie-ide-plugin.html | Confirms IDE plugin has no hooks/lifecycle-event system; documents `.junie/AGENTS.md` (current) vs `.junie/guidelines.md` (deprecated legacy) and `.junie/mcp/mcp.json` | 2026-08-14 |
| https://junie.jetbrains.com/docs/junie-headless.html | Headless/CI invocation (`junie --auth=... "<prompt>"`), confirms CI/CD design intent; trust-gate rollout-toggle language for non-interactive modes; mentions hooks alongside MCP/agents/skills/guidelines as things withheld from untrusted projects | 2026-08-14 |
| https://junie.jetbrains.com/docs/environment-variables.html | Full env var list (`JUNIE_*`); confirms no hook-specific or trust-specific env vars beyond `JUNIE_CONFIG_LOCATION`/`JUNIE_CONFIG_DEFAULT_LOCATIONS` | 2026-08-14 |
| Search snippet quoting `junie-cli-configuration.html` (trust section) | Project-trust key storage (OS keychain / Secret Service / owner-only file), `<Junie Home>/trust` marker directory, trust-decision prompt options | 2026-08-14 |
| https://blog.jetbrains.com/junie/2026/03/junie-cli-the-llm-agnostic-coding-agent-is-now-in-beta/ (title/snippet only, not fully fetched) | Junie CLI promoted EAP → Beta, dated March 2026 | 2026-08-14 (search snippet) |
| https://youtrack.jetbrains.com/projects/JUNIE (search only) | Confirms JUNIE is the live YouTrack project for feature requests; no specific hooks-request issue found (feature already shipped, so absence of a request issue is expected, not evidence of anything) | 2026-08-14 |
| https://youtrack.jetbrains.com/issue/JUNIE-618/Support-AGENTS.md ; JUNIE-84 (Advanced AI rules) [context only, not hooks] | Adjacent/non-hook context confirming AGENTS.md and "rules" are separate, older feature threads — not hooks | 2026-08-14 (search snippet) |
| www.jetbrains.com/help/junie/ | 301-redirects to `junie.jetbrains.com/docs/` as of this fetch — confirms JetBrains has unified the help URL onto the CLI docs site | 2026-08-14 |

### Notes on evidence quality

- All hooks-specific quotes above came from **WebFetch summaries of the single primary
  source page** `junie-cli-hooks.html`, run across five separate targeted fetches to
  triangulate exact wording per section (the tool summarizes with a small model rather
  than returning raw HTML, and no direct `curl` egress was available in this sandbox to
  get raw text). Every fetch of that URL was internally consistent across passes (schema,
  event list, timeouts, and quotes did not contradict each other run to run), which is
  the best corroboration available without raw-HTML access — but strings should still be
  re-verified against the live page before being encoded verbatim into a schema/parser.
- The March-2026 Beta-announcement blog post and the two YouTrack issue titles were only
  seen via **search-result snippets**, not a full WebFetch — flagged accordingly above.
- No `[unofficial]` (third-party/blog/Reddit) sources were used as fact anywhere in this
  file; a Datadog Security Labs piece on cross-agent "trust" surfaced during search but
  was not used because it read as a general cross-vendor piece, not Junie-specific
  primary evidence — excluded rather than risk misattributing generic language to Junie.
