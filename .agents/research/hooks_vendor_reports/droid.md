# Droid CLI (Factory.ai) — hook / lifecycle-event mechanism

Research date: 2026-08-14. Current published `droid` npm version at time of research: **0.196.0**
(published 2026-08-14T02:07:40Z, per `npm view droid version` / `npm view droid time.modified`).

## 1. Existence & name

**Exists. Vendor calls it "Hooks."** Documented as a first-class harness capability alongside
MCP, Skills, Subagents, and Plugins ("Customize the Harness" nav section).

- No beta/experimental/flag-gated language anywhere in the hooks docs. Treated as a stable,
  shipped feature (see `docs.factory.ai/cli/configuration/hooks-guide`, fetched 2026-08-14 — full
  page reproduced verbatim below).
- No deprecation notice.
- **Since which version — NOT DOCUMENTED explicitly** (no "added in vX" marker on the docs page),
  but the public changelog (`docs.factory.ai/changelog/release-notes`, fetched 2026-08-14) shows
  the feature already mature by **v0.111.0 (2026-04-28)** ("Refreshed hooks UI") and under active
  iteration through mid-2026:
  - v0.111.0 (2026-04-28) — "Refreshed hooks UI - Updated UI for managing hooks"
  - v0.143.0 (2026-06-08) — "Quieter hooks - Hooks can hide their output block in the TUI by
    setting `suppressOutput`"
  - v0.152.0 (2026-06-18) — "Event hooks - Multiple hooks configured for the same event are now
    combined correctly"
  - v0.161.0 (2026-06-29) — "Hooks manager overhaul - Redesigned the hooks manager for viewing and
    configuring your hooks"
  - v0.171.0 (2026-07-13) — "Hook execution - Hooks now run exactly once per event"
  - v0.176.0 (2026-07-20) — "Resumed hooks - Hook rows now render correctly after you resume a
    session"
  - v0.177.0 (2026-07-21) — "Hook display - Hook metadata now stays on a single row"
  - v0.182.0 (2026-07-28) — "Notification hook - The `idle_prompt` Notification hook now fires on
    cancelled turns"
  - The page's visible version span runs from v0.105.0 (2026-04-20) up to v0.195.0 (2026-08-13);
    I did not find any entry documenting the *initial introduction* of hooks, implying it predates
    v0.105.0. **NOT DOCUMENTED**: exact ship date/version.
  - I found no entry (through v0.196.0) fixing the two open bugs described in §9 below (project-
    scope `hooks.json` not loading; `DROID_PLUGIN_ROOT` sentinel leak) — searched the v0.180.0–
    v0.196.0 range specifically for "hooks.json", "DROID_PLUGIN_ROOT", "plugin root" and found
    nothing.

## 2. Config location(s)

From `docs.factory.ai/cli/configuration/hooks-guide` (verbatim table):

| Scope | File | Notes |
|---|---|---|
| User | `~/.factory/hooks.json` | Applies across your projects. |
| Project | `.factory/hooks.json` | Commit to share with teammates. |
| Enterprise | Org-managed settings | Loaded from Enterprise Controls and managed settings. |
| Legacy | `.factory/hooks/hooks.json` | Still loads. The next save writes `.factory/hooks.json` and archives the old file as `hooks/hooks.migrated.json`. |

> "If `hooks.json` is absent, Droid also reads hook declarations from the `hooks` key in the
> matching `settings.json`."

`settings.json` itself (`docs.factory.ai/cli/configuration/settings`, fetched 2026-08-14):

| OS | Location |
|---|---|
| macOS / Linux | `~/.factory/settings.json` |
| Windows | `%USERPROFILE%\.factory\settings.json` |
| Project | `<project>/.factory/settings.json` |

Local overrides: `~/.factory/settings.local.json` and `<project>/.factory/settings.local.json`,
merging "on top of the corresponding `settings.json` at the same level and follow[ing] the same
hierarchy precedence." Recommended for `.gitignore`.

**No environment variable relocates either file** — the settings page has no such env var (I
looked specifically for one; none documented). `AGENTS.md` note (adjacent, not a hook mechanism):
uses `/harness/agents-md`, unrelated file.

**Directory convention for auto-discovery**: yes — plugins ship `hooks/hooks.json` at their
plugin root, auto-loaded when the plugin is enabled (see §3, Plugin hooks).

**Merging across sources**: Not "one wins" — **additive/accumulative**, confirmed on the
Enterprise Controls page (`docs.factory.ai/enterprise/hierarchical-settings-and-org-control`,
fetched 2026-08-14):

> "the full order, highest priority first, is: 1) Org (and org plugins), 2) Runtime, 3) Folder,
> 4) Project, 5) User, 6) Dynamic, 7) BuiltIn"
>
> "Array fields accumulate across levels. Org entries are always present; project, folder, and
> user levels can add more without removing or weakening higher-level entries."

For hooks specifically: "Org hooks always load unless hooks are globally disabled, and lower
levels cannot remove them." With `allowManagedHooksOnly: true`, only org + org-enabled-plugin
hooks load; user/project hooks are dropped entirely (not merged).

Plugin hooks "merge... with user, project, and managed hooks when the plugin is enabled" (exact
merge algorithm beyond "accumulate" — **NOT DOCUMENTED** at the field level, e.g. no stated
tie-break when two sources register hooks on the same event+matcher; behavior observed is that
they simply all run, per "Multiple hooks configured for the same event are now combined
correctly" changelog entry v0.152.0).

**Snapshot-at-startup**: yes, but only mentioned in the security section (see §9), not the config
section: "Droid snapshots hooks at startup and warns when hooks are modified externally."

## 3. Config schema — verbatim

**It is a named map, keyed directly by event name, whose value is an ARRAY of matcher-group
objects** — not a flat array of hook entries, and not a map keyed by hook id.

Standalone `hooks.json` (`~/.factory/hooks.json` shown, project file is the same shape unwrapped):

```json
{
  "PreToolUse": [
    {
      "matcher": "Execute",
      "commandRegex": "^git ",
      "hooks": [
        {
          "type": "command",
          "command": "/usr/local/bin/audit-git-command.sh",
          "timeout": 30
        }
      ]
    }
  ]
}
```

Inside `settings.json`, the identical event-map is wrapped one level deeper under a top-level
`"hooks"` key:

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Execute",
        "commandRegex": "^git ",
        "hooks": [
          {
            "type": "command",
            "command": "/usr/local/bin/audit-git-command.sh",
            "timeout": 30
          }
        ]
      }
    ]
  }
}
```

Field-by-field (verbatim from the docs' own table):

| Field | Required | Source-validated behavior |
|---|---|---|
| `matcher` | No | Empty, omitted, or `*` matches everything. Exact strings match one tool or lifecycle matcher. Regex patterns are supported and are case-sensitive. |
| `commandRegex` | No | Additional regex filter for Execute commands. It matches the actual shell command string when Droid has one. Invalid regex values are skipped. |
| `hooks` | Yes | Array of hook commands for the matcher group. |
| `type` | Yes | Currently only `"command"` is supported. |
| `command` | Yes | Shell command executed with JSON hook input on stdin. |
| `timeout` | No | Per-command timeout in seconds. Defaults to `60`. |

Common tool matchers (verbatim): `Execute`, `Read`, `Edit`, `Create`, `ApplyPatch`, `LS`, `Glob`,
`Grep`, `Task`, `FetchUrl`, `WebSearch`. MCP tools use `mcp__<server>__<tool>`; `mcp__.*` matches
all MCP tools.

**Stable identity field for a third-party installer — NONE.** There is no `id`, `name`, or
`description` field anywhere in a hook entry, matcher group, or the top-level event array. A
matcher group is identified only by its position in the array plus its `matcher`/`commandRegex`
content. **This is a real gap** for an installer (like grim) that wants to idempotently own,
update, and remove exactly one entry — it would have to fingerprint its own entries by convention
(e.g. a recognizable `command` string prefix/marker), since JSON supports no comments and the
schema has no identity field. Confirmed absent by reading the full field table above and the
three JSON examples in the docs (quickstart, plugin, org-managed) — none carry an id/name field.

## 4. Event catalogue — verbatim

From the docs' "Hook events" table:

| Event | Runs when | Matcher / key fields | Common use |
|---|---|---|---|
| `PreToolUse` | After Droid builds tool parameters and before the tool runs. | `tool_name`, `tool_input`; matcher usually targets a tool such as `Execute` or `Edit`. | Block risky operations, approve safe tools, rewrite tool input. |
| `PostToolUse` | Immediately after a tool completes. | `tool_name`, `tool_input`, `tool_response`; same tool matchers as PreToolUse. | Format files, run validation, add feedback after an edit. |
| `UserPromptSubmit` | Before Droid processes a submitted user prompt. | `prompt`, `has_images`. | Validate prompts or inject extra context. |
| `Notification` | When Droid sends a notification. | `message`, `notification_type` (`permission_prompt`, `idle_prompt`, `auth_success`, `elicitation_dialog`). | Desktop alerts or compliance logging. |
| `Stop` | When the main Droid is about to finish responding. | `stop_hook_active`, `tool_execution_count`, `elapsed_time`. | Require final checks or continue with follow-up instructions. |
| `SubagentStop` | When a Task-launched sub-droid finishes. | `task_name`, `task_result`, `task_error`, `stop_hook_active`. | Validate subagent output or request more work. |
| `PreCompact` | Before manual or automatic compaction. | `trigger` (`manual`/`auto`), `custom_instructions`, `message_count`, `estimated_tokens`. | Save context or add compaction guidance. |
| `SessionStart` | When Droid starts, resumes, clears, or starts after compaction. | `source` (`startup`, `resume`, `clear`, `compact`), plus optional prior session IDs. | Load local context at session start. |
| `SessionEnd` | When a Droid session ends. | `reason` (`clear`, `logout`, `prompt_input_exit`, `other`), `session_duration_ms`, `message_count`. | Cleanup, audit logs, session summaries. |

Notification sub-types table:

| Type | Sent when |
|---|---|
| `permission_prompt` | Droid is waiting for permission to run an action. |
| `idle_prompt` | Droid is waiting for user input, including immediately after the user cancels a turn. |
| `auth_success` | An authentication flow succeeds. |
| `elicitation_dialog` | Droid needs structured input in an elicitation dialog. |

Grouped per the brief's taxonomy: session lifecycle = `SessionStart`, `SessionEnd`; prompt submit
= `UserPromptSubmit`; pre/post tool use = `PreToolUse`, `PostToolUse`; file edit = covered under
tool matchers (`Edit`, `Create`, `ApplyPatch`) inside Pre/PostToolUse, not a separate event;
command execution = `Execute` matcher, same mechanism; notification = `Notification`; stop/finish
= `Stop`, `SubagentStop`; compaction = `PreCompact`; subagent = `SubagentStop`; error = **no
dedicated `Error`/`OnError` event exists** (NOT DOCUMENTED / does not exist — errors surface only
via non-zero exit codes from tool-use hooks, not as their own lifecycle event).

Important nuance directly quoted: "Cancellation emits the informational `Notification` hook
instead of `Stop`, so a hook cannot override the user's decision to stop the turn."

## 5. Invocation

- **Execution model**: shell command string (`type: "command"`), not a JS/TS module import, not
  an HTTP endpoint, not an argv array — literally "Shell command executed with JSON hook input on
  stdin."
- **Working directory**: "Hooks execute from Droid's current working directory, which can differ
  from your repository root." Docs explicitly warn to use absolute paths or
  `"$FACTORY_PROJECT_DIR"/path/to/script.sh` rather than relying on relative paths.
- **Shell used** — NOT DOCUMENTED (no statement of `/bin/sh` vs the user's `$SHELL` vs a specific
  shell). **`$PATH` handling** — NOT DOCUMENTED beyond "hooks inherit your local environment"
  (§9), which implies the invoking process's `$PATH` is inherited, but this is inferred, not
  stated outright.
- **Timeout**: per-command `timeout` field, seconds, default `60`. Global default override —
  NOT DOCUMENTED (no global timeout setting found; it's set per hook-command entry only).
- **Concurrency / ordering**: "Multiple hooks configured for the same event are now combined
  correctly" (changelog v0.152.0) and "Hooks now run exactly once per event" (changelog v0.171.0)
  imply an internal execution/dedup model, but the docs do not state whether multiple matched
  hooks for one event run in parallel or in series, nor whether matcher-group order in the array
  is the execution order. **NOT DOCUMENTED.**
- **Debugging**: "Run `droid --debug` to inspect hook matching and execution details."
- Plugin hook commands support extra variable expansion: `${DROID_PLUGIN_ROOT}`,
  `$DROID_PLUGIN_ROOT`, `${CLAUDE_PLUGIN_ROOT}`, `$CLAUDE_PLUGIN_ROOT` (the latter two for
  Claude Code plugin compatibility), expanded "to the installed plugin cache path when loading
  plugin hooks." **This expansion is confirmed broken at runtime as an env var** — see §9/§10.

## 6. Input payload — verbatim

Every hook receives JSON **on stdin**. Common fields are *also* exposed as environment variables
"when their values are strings, booleans, or numbers" (i.e., nested objects like `tool_input` are
NOT flattened into env vars, only scalar top-level fields are — this is my inference from the
wording, not an explicit list of which fields are env-exposed).

Base shape (verbatim TypeScript-style block from the docs):

```typescript
{
  session_id: string
  transcript_path: string
  cwd: string
  permission_mode: "off" | "spec" | "auto-low" | "auto-medium" | "auto-high"
  hook_event_name: string
  message_id?: string
}
```

Tool-hook example (verbatim JSON from docs, a real `PreToolUse` payload):

```json
{
  "session_id": "abc123",
  "transcript_path": "/Users/.../.factory/projects/.../session.jsonl",
  "cwd": "/Users/me/project",
  "permission_mode": "off",
  "hook_event_name": "PreToolUse",
  "tool_name": "Create",
  "tool_input": {
    "file_path": "/Users/me/project/file.txt",
    "content": "file content"
  }
}
```

`PostToolUse` additionally includes `tool_response`. "The exact shape of `tool_input` and
`tool_response` depends on the tool" — i.e. **not one fixed schema**, it's tool-specific and
otherwise undocumented per-tool.

No template-interpolation syntax (no `$TOOL_NAME`/`{{file}}` placeholders in the command string
itself) is documented — the *only* documented command-string variables are the two working-
directory helpers (`$FACTORY_PROJECT_DIR`) and the plugin-root variables above. All event data
proper arrives via stdin JSON (and mirrored scalar env vars), not command-string templating.

## 7. Output / response contract — verbatim

| Output | Effect |
|---|---|
| Exit code `0` | Success. For `UserPromptSubmit` and `SessionStart`, stdout can add context. For other events, stdout is visible in transcript views. |
| Exit code `2` | Blocking or corrective feedback. `PreToolUse` blocks the tool call, `PostToolUse` and `Stop` feed stderr back to Droid, and `UserPromptSubmit` blocks prompt processing. Other lifecycle events surface stderr to the user. |
| Any other non-zero exit | Non-blocking error. Droid records stderr and continues where the event permits. |
| JSON `continue: false` | Stops processing after the hook. `stopReason` can explain why to the user. |
| JSON `suppressOutput: true` | Hides successful hook output from the main chat view while preserving it in the detailed transcript. |

**PreToolUse-specific JSON response** (verbatim example):

```json
{
  "hookSpecificOutput": {
    "hookEventName": "PreToolUse",
    "permissionDecision": "allow",
    "permissionDecisionReason": "Documentation reads are safe",
    "updatedInput": {
      "file_path": "/Users/me/project/README.md"
    }
  }
}
```

| Decision | Effect |
|---|---|
| `allow` | Allows the tool call and can bypass the normal permission prompt. |
| `deny` | Blocks the tool call and sends the reason to Droid. |
| `ask` | Forces a user confirmation prompt. |

`updatedInput` can rewrite tool parameters pre-execution.

**Other events' JSON fields** (verbatim table):

| Event | JSON fields |
|---|---|
| `PostToolUse` | `decision: "block"` sends `reason` back to Droid after the tool ran. `hookSpecificOutput.additionalContext` adds more context. |
| `UserPromptSubmit` | `decision: "block"` prevents the prompt from being processed and shows `reason` to the user. `additionalContext` is appended when not blocked. |
| `Stop` and `SubagentStop` | `decision: "block"` prevents stopping. Include `reason` so Droid knows what to do next. |
| `SessionStart` | `hookSpecificOutput.additionalContext` is appended to the new session context. |
| `SessionEnd` | Cannot block session termination. Use it for cleanup and logging. |

There is no `systemMessage` field documented (unlike some competing tools' hook contracts) —
**NOT DOCUMENTED / apparently absent**; the closest analog is `stopReason` (on `continue: false`)
and per-event `reason` fields.

**stdout/stderr visibility**: stdout is shown in the transcript (and to the model as context for
`UserPromptSubmit`/`SessionStart`); `showHookOutput` (a `settings.json` key, default unset/false)
additionally surfaces stdout+stderr "in the session transcript for debugging." `suppressOutput`
hides it from the main chat view but keeps it in the detailed transcript. So: hook output can
reach the user (transcript), the model (as injected context, for the two events named), or be
suppressed — governed by the fields above, not a fixed single destination.

## 8. Reliability & limits

- Default timeout **60 seconds**, per-hook-command override via `timeout` (seconds).
- No documented global default override, no documented max ceiling.
- Non-zero-but-not-2 exit: "non-blocking error... Droid records stderr and continues where the
  event permits" — i.e. execution is not aborted outright except where the event semantics say a
  hook can block (PreToolUse/PostToolUse/Stop/UserPromptSubmit via exit 2 or `decision: "block"`).
- Malformed JSON output on stdout — **NOT DOCUMENTED** (no stated fallback behavior; presumably
  treated as plain text/no-op given "stdout is visible in transcript views" as the default path).
  Missing binary / command-not-found — **NOT DOCUMENTED** explicitly, but would surface as a
  shell-exec failure captured the same way as a non-zero exit (inference, not a quote).
  Malformed hook *config* JSON — **NOT DOCUMENTED**, no stated validation-error behavior for the
  hooks.json file itself.
- Parallel vs. serial execution across multiple matched hooks — **NOT DOCUMENTED** (see §5).
- Blocking vs. fire-and-forget: **blocking** — the whole premise of exit-code/JSON control (deny/
  ask/block) requires Droid to wait for the hook to finish before proceeding, at least for
  PreToolUse/PostToolUse/Stop/UserPromptSubmit. One documented exception in the changelog (not the
  hooks-guide page itself): "background processes spawned in `droid exec` now keep running after
  `droid exec` exits" — this describes child processes *spawned by* a hook/tool outliving the
  parent run, not the hook invocation itself being non-blocking.
- SessionStart hooks: changelog notes "SessionStart hooks no longer block session initialization
  when hung" (a reliability fix; exact version not captured — found via search summary, not
  independently re-verified against a specific version header, flagging as **medium-confidence**).

## 9. Security posture

Direct quotes, `docs.factory.ai/cli/configuration/hooks-guide`:

> **Warning callout**: "Hooks run automatically with your local environment and credentials.
> Review every hook command before registering it, use absolute script paths, and test in a safe
> environment first."

Dedicated "Security considerations" section (verbatim bullets):

- "Treat hook input as untrusted JSON. Validate and sanitize paths, prompts, and command strings."
- "Quote shell variables, for example `"$FACTORY_PROJECT_DIR"`, to avoid word splitting."
- "Block path traversal and sensitive paths such as `.env`, `.git/`, credentials, and deployment
  secrets."
- "Prefer small scripts checked into `.factory/hooks/` over long inline one-liners."
- "Test hooks manually before enabling them for a team or organization."
- "Remember that hooks inherit your local environment. Avoid commands that exfiltrate data or
  mutate systems unexpectedly."
- "Droid snapshots hooks at startup and warns when hooks are modified externally. Review changes
  in the `/hooks` UI before relying on them in the current session."

No pre-run approval prompt / allowlist specific to *installing* a hook is documented beyond the
above snapshot-and-warn behavior and the global `hooksDisabled` kill switch (`settings.json`,
default `false`). There is no per-hook "trust this command" gate described — trust is
scope-based (user/project/org file location) plus the startup-snapshot warning, not a runtime
confirmation dialog per hook the way tool-call permissions work.

**Confirmed live bugs (primary source: GitHub, `Factory-AI/factory`, public, org-owned,
`has_issues: true`, both issues state `OPEN` as of 2026-08-14, filed 2026-08-03, no maintainer
response/label yet — treat as unconfirmed-by-vendor but reproducible, dated, primary-source
reports)**:

1. **[github.com/Factory-AI/factory#3](https://github.com/Factory-AI/factory/issues/3)** —
   "`.factory/hooks.json` is silently never read at project scope (documented primary location)."
   Reporter's reproduction table (droid 0.186.0, macOS, `droid exec` mode): a canary
   `matcher: "*"` `PreToolUse` hook fires **0** times when placed in `.factory/hooks.json`
   (project), `~/.factory/hooks.json` (user), or `.factory/hooks/hooks.json` (legacy) — and
   fires **1** time only when the identical declaration is placed in the `hooks` key of
   `.factory/settings.json`. Quoted: "This is a security-relevant failure: a hook is a policy
   control (test-lock, path guard, etc.). An operator following the docs will believe a control
   is installed when it is not." A follow-up comment (same day) reports this is a **regression**:
   the identical project-scope `.factory/hooks.json` canary fired correctly on droid **0.180.0**
   and went silent by **0.186.0** — "whatever changed the project-scope hook loader between 0.180
   and 0.186 dropped `.factory/hooks.json` from the read path while keeping the `settings.json`
   `hooks` key and plugin-shipped `hooks/hooks.json` working." I independently checked the public
   changelog v0.180.0→v0.196.0 (today's latest) for any fix and found none mentioning
   `hooks.json` or project-scope loading — **the bug appears to still be open on the current
   0.196.0 release**, though I did not personally reproduce it (no droid installation available
   in this research environment; relying on the GitHub report as primary source per the evidence
   rules, cross-checked only via the changelog's silence on a fix).

2. **[github.com/Factory-AI/factory#5](https://github.com/Factory-AI/factory/issues/5)** —
   "`DROID_PLUGIN_ROOT` env var in plugin hooks is the literal sentinel string
   `/PLUGIN_ROOT_NOT_EXPANDED_ERROR`, not the plugin's install path." Command-string substitution
   (`${DROID_PLUGIN_ROOT}` inside the JSON `command` field) works; a script that instead reads
   `os.environ["DROID_PLUGIN_ROOT"]` at runtime gets the literal sentinel string. Reproduces on
   both 0.180.0 and 0.186.0 per a same-day follow-up comment — "stable across" both versions,
   unlike issue #3.

These matter directly for grim: they show the vendor's own documented "primary" project-scope
file is not currently trustworthy, and that plugin-root env-var expansion (as opposed to
command-string expansion) cannot be relied on.

## 10. Third-party installability

**Yes, by editing files — no vendor CLI or UI is required to install a hook.** The whole
mechanism is declarative JSON on disk; `/hooks` in the TUI is a convenience editor over the same
files, not the only way to write them (confirmed by the docs showing the manager saves "the same
unwrapped structure" a human could hand-write).

Practical guidance given the bugs in §9: **the reliable installation target today is the `hooks`
key inside `.factory/settings.json` (project or user scope), not the dedicated `.factory/
hooks.json` file** — the latter is documented as primary but is reported broken at project scope
on the currently-shipping CLI line (0.186.0–0.196.0, unresolved per changelog review). This is
actually favorable for grim specifically: grim already "splices JSON... config files in place,
preserving every byte outside the managed member" (per its own architecture) — settings.json is
exactly that kind of file, so grim's existing JSON-splice machinery is a better fit than
maintaining a second `hooks.json` file that droid may not read.

**Snapshot-at-startup gotcha — confirmed**: "Droid snapshots hooks at startup and warns when
hooks are modified externally." This means a hook grim installs (or updates) while a droid
session is already running will not take effect in that session — it needs a new session (the
docs frame it as a warning-and-review UX in `/hooks`, not an automatic hot-reload), which is the
functional equivalent of "needs a restart" for the purposes of an external installer. No
statement of whether a *new* `droid` invocation (e.g. a fresh `droid exec` in CI) re-reads the
file fresh each time — likely yes, since each CLI invocation is a new process, but this is
inference, not a quote.

**Enterprise carve-out**: `allowManagedHooksOnly: true` makes user/project-level hook files
(including anything grim would write) inert, restricting to org-managed settings and org-enabled
plugins only. An installer cannot detect or override this locally by design.

## 11. Trampoline viability

**Verdict: mechanically very good fit, with one identity gap.**

Supporting facts:
- Invocation is *always* `type: "command"` → an arbitrary shell string. A generic
  `grim hook run --client droid --event <E> --matcher <M>` invocation is a completely ordinary
  value for the `command` field — no JS/TS module loading, no in-process function requirement, no
  HTTP endpoint needed.
- Payload delivery is stdin JSON with a stable base envelope (`session_id`, `cwd`,
  `hook_event_name`, `permission_mode`, ...) plus event-specific fields — trivial for a trampoline
  binary to parse once and dispatch on `hook_event_name`.
- Response contract is exit code + optional JSON on stdout — trivial for a trampoline to emit;
  the per-event field sets (`hookSpecificOutput.permissionDecision`, `decision: "block"`,
  `additionalContext`, `continue`/`stopReason`, `suppressOutput`) are all plain JSON grim's own
  process can construct directly from whatever the *portable* hook schema decides.
- `$FACTORY_PROJECT_DIR` and (for plugin-shipped hooks) `$DROID_PLUGIN_ROOT` are the only
  command-string template variables — grim would supply its own fixed arguments/env instead of
  relying on droid-side templating, so this is a non-issue.

Blockers / caveats to design around:
1. **No id/name field on a hook entry (§3).** Grim cannot ask droid "which entries are mine" —
   it must own identification by convention (e.g., recognize entries whose `command` starts with
   a fixed `grim hook run` prefix, or reserve a specific matcher-group position) to update/remove
   idempotently without clobbering hand-written neighbor entries in the same event array.
2. **Two legal write targets, only one currently reliable (§9, §10).** A naive implementation
   targeting the documented-primary `.factory/hooks.json` would silently no-op at project scope
   on current droid releases; grim should target the `hooks` key of `.factory/settings.json`.
   This should be re-verified against whatever droid version is current when grim implements this
   (the bug may be fixed by then — no fix visible as of v0.196.0 today).
3. **Execution/ordering semantics under-documented (§5, §8)** — if grim ever materializes more
   than one hook into the same event+matcher slot (e.g. two different grim-managed packages both
   wanting `PostToolUse`/`Edit`), the relative order and parallelism of execution across
   matcher-group entries within one event array is not specified by the vendor. Low risk for a
   single trampoline command per event (grim could register exactly one matcher-group per event
   that itself fans out internally), but worth being deliberate about.
4. **Snapshot-at-startup (§10)** means grim-driven install/update is not "live" for an
   already-running interactive session — acceptable for a package manager (matches how grim
   already treats other native config: installed state takes effect on next invocation).
5. Headless applicability: the two GitHub bug reports were both reproduced under `droid exec`
   mode, which confirms (as a side effect, not a direct doc statement) that **the same
   `hooks.json`/`settings.json` hook mechanism is active in headless/CI `droid exec` runs**, not
   just the interactive TUI. The separate JSON-RPC `droid.session_notification` /
   `droid.request_permission` / `droid.ask_user` stream documented for `droid exec
   --output-format stream-jsonrpc` (`docs.factory.ai/droid-exec/overview`) is a **different,
   unrelated mechanism** — a client-side control protocol for building custom UIs around a
   long-lived droid subprocess, not user-supplied deterministic code execution, and out of this
   brief's scope by the brief's own definition of "hooks." No blocker for grim either way, since
   grim would target the hooks.json/settings.json mechanism, not the JSON-RPC stream.

Net: no fundamental blocker to a generic trampoline; the main design decision is (a) write target
(`settings.json` `hooks` key, not `hooks.json`) and (b) an identity convention for owning entries
in an array that has none natively.

## Disambiguation (adjacent, NOT hooks, per brief's scope boundary)

- **AGENTS.md** (`/harness/agents-md`) — repository instructions/conventions, static text, not
  executed code.
- **Custom Slash Commands**, **Skills**, **Subagents**, **MCP** — separate "Customize the
  Harness" nav siblings to Hooks; not lifecycle-event code execution.
- **Plugins** (`/harness/plugins`) — a packaging/distribution mechanism (commands + skills +
  droids + hooks + mcp.json bundled together with a `.factory-plugin/plugin.json` manifest); a
  plugin can *ship* hooks (see §3 Plugin hooks) but "plugin" itself is not the hook mechanism —
  it's one more scope hooks can be declared at. Manifest schema (verbatim):
  ```json
  {
    "name": "my-plugin",
    "description": "A helpful plugin description",
    "version": "1.0.0",
    "author": { "name": "Your Team" },
    "homepage": "https://github.com/your-org/my-plugin",
    "repository": "https://github.com/your-org/my-plugin",
    "license": "MIT",
    "keywords": ["review", "security"]
  }
  ```
- **`droid exec` JSON-RPC control stream** (`droid.session_notification`,
  `droid.request_permission`, `droid.ask_user`) — a headless client-integration protocol, not
  user-supplied hook code (see §11 point 5).
- **Enterprise "managed settings"** — a distribution/precedence mechanism for org-pushed
  `settings.json` (including its `hooks` key), not a distinct hook mechanism.

## Sources

| URL | What it establishes | Fetched |
|---|---|---|
| https://docs.factory.ai/cli/configuration/hooks-guide | Full hooks schema, event catalogue, invocation, input/output contract, security section, debugging table, plugin/org hook sections — verbatim, primary source | 2026-08-14 |
| https://docs.factory.ai/cli/configuration/settings | `settings.json` file paths (all OS/scopes), `settings.local.json` override merging, `hooksDisabled`/`showHookOutput` keys, full settings key list, "Legacy Droid YAML" note that hooks/MCP/skills replace `.droid.yaml` | 2026-08-14 |
| https://docs.factory.ai/harness/hooks | Confirmed same content family as `/cli/configuration/hooks-guide` (nav alias); no additional stability/version markers found | 2026-08-14 |
| https://docs.factory.ai/enterprise/hierarchical-settings-and-org-control | Verbatim precedence order (Org > Runtime > Folder > Project > User > Dynamic > BuiltIn), array-accumulation merge semantics, `allowManagedHooksOnly` behavior | 2026-08-14 |
| https://docs.factory.ai/harness/plugins | Plugin directory layout, `.factory-plugin/plugin.json` manifest schema (verbatim), plugin-root hook variable expansion, install scopes (`--scope user`/`--scope project`), official marketplace `Factory-AI/factory-plugins` | 2026-08-14 |
| https://docs.factory.ai/droid-exec/overview | Headless mode output formats (text/json/stream-jsonrpc); JSON-RPC notification/request methods (`droid.session_notification`, `droid.request_permission`, `droid.ask_user`); confirms this is a distinct mechanism from hooks, no hook/webhook mention on the page itself | 2026-08-14 |
| https://docs.factory.ai/droid-cli/cli-reference | Binary name `droid` (`droid.exe` on Windows), install via Homebrew/npm (`npm install -g droid@<version>`), `--debug` flag context (cross-referenced from hooks-guide), `/hooks`/`/plugins` slash commands, `droid exec`, `-o/--output-format` values including `stream-jsonrpc` | 2026-08-14 |
| https://docs.factory.ai/changelog/release-notes | Version-dated history of hook feature changes v0.105.0 (2026-04-20) through v0.195.0/v0.196.0 (2026-08-13/14); no entry found fixing the two GitHub-reported bugs through the latest version | 2026-08-14 |
| https://github.com/Factory-AI/factory (repo metadata via `gh api repos/Factory-AI/factory`) | Confirms public, org-owned, issues-enabled repo; created 2026-07-29, 9 open issues, no source language (not the CLI's source code — appears to be an issue-tracker/community repo for the product) | 2026-08-14 |
| https://github.com/Factory-AI/factory/issues/3 | OPEN primary-source bug report: project-scope `.factory/hooks.json` silently unread on droid 0.186.0; regression vs. 0.180.0 per follow-up comment; both dated 2026-08-03 | 2026-08-14 |
| https://github.com/Factory-AI/factory/issues/5 | OPEN primary-source bug report: `DROID_PLUGIN_ROOT` env var resolves to literal sentinel `/PLUGIN_ROOT_NOT_EXPANDED_ERROR` at runtime; reproduces on both 0.180.0 and 0.186.0; dated 2026-08-03 | 2026-08-14 |
| `npm view droid version` / `npm view droid versions --json` / `npm view droid time.modified` (npm registry, via Bash) | Current published version 0.196.0, published 2026-08-14T02:07:40Z; confirms version numbering matches changelog page's own version headers | 2026-08-14 |
