# Cursor — Hooks research

Client: **Cursor** (IDE + `cursor-agent` CLI). Research date: 2026-08-14.

## 1. Existence & name

Cursor ships a feature it calls, simply, **Hooks** (docs group it into "Agent Hooks",
"Tab Hooks", and an app-lifecycle hook; there is no separate marketing name per group).
Docs page: <https://cursor.com/docs/hooks>. Broader framing: a "Plugins" packaging concept
(<https://cursor.com/docs/reference/plugins>) that says verbatim: *"Plugins package rules,
skills, agents, commands, MCP servers, and hooks into distributable bundles"* and *"Hooks
are automation scripts triggered by agent, Tab, or workspace events."* — i.e. inside Cursor's
own model, "hook" is already one of the artifact kinds a "plugin" bundles, which is a strong
naming precedent for grim's own `hook` artifact kind.

**Introduced:** Cursor 1.7, announced 2025-09-29 in the official changelog
(<https://cursor.com/changelog/1-7>, fetched 2026-08-14). Verbatim: *"You can now observe,
control, and extend the Agent loop using custom scripts. Hooks give you a way to customize
and influence Agent behavior at runtime."* Use cases named: *"audit Agent usage, block
commands, or redact secrets from context."*

**Beta status:** the 1.7 changelog explicitly states *"It's still in beta and we'd love to
hear your feedback."* As of the current docs page (fetched 2026-08-14, ~11 months later)
there is **no** Beta/Experimental badge or wording anywhere on <https://cursor.com/docs/hooks>
— checked explicitly, "NOT FOUND" for a beta marker near the title or in a sidebar tag. I
found no explicit "hooks are now GA/stable" announcement either. **Best-supported read:**
quietly graduated from beta between Sept 2025 and Aug 2026, but this is an inference from
silence, not a quoted statement — flagging as medium confidence.

Practical counter-signal to "stable," from Cursor's own community forum (staff-confirmed,
see §8): the `permission: "ask"`/`"allow"` verdicts from hooks are still not honored
correctly as of at least Cursor 2.4.21 and are the subject of an open forum Feature Request
dated after that (*"Support authoritative allow, deny, and ask verdicts from hooks"*,
<https://forum.cursor.com/t/support-authoritative-allow-deny-and-ask-verdicts-from-hooks/161342>,
[unofficial], undated in the snippet but younger than the 2.4.21 bug thread). Also, an
`afterAgentResponse`/`afterAgentThought` gap in the headless CLI was confirmed as a live bug
by a Cursor staff member ("Colin") on 2026-03-30 (see §8). Net: shipped, documented,
non-beta-labeled, but with acknowledged rough edges — I would not call this a mature/settled
contract yet.

## 2. Config location(s)

Exact paths (from <https://cursor.com/docs/hooks>, fetched 2026-08-14):

| Scope | Path |
|---|---|
| Project | `<project-root>/.cursor/hooks.json` |
| Global / user | `~/.cursor/hooks.json` |
| Enterprise (macOS) | `/Library/Application Support/Cursor/hooks.json` |
| Enterprise (Linux/WSL) | `/etc/cursor/hooks.json` |
| Enterprise (Windows) | `C:\ProgramData\Cursor\hooks.json` |
| Plugin-bundled | `<plugin-dir>/hooks/hooks.json` (per the Plugins reference page) |

Format: **JSON** (not JSONC/TOML/YAML). No env var was found that relocates the hooks.json
path itself — checked explicitly, NOT FOUND on the hooks doc page. (Contrast: env vars *fed
into* the hook process, §6, are a separate thing and do exist.)

**Directory/auto-discovery convention:** plugins carry their own `hooks/hooks.json`; the
`workspaceOpen` hook's response schema even has a `pluginPaths: string[]` output field
described as *"Absolute paths to plugin directories to load"* — i.e. a hook can itself
register more plugin (and by extension hook) sources at workspace-open time. Beyond that,
no wildcard-glob directory scan (e.g. `hooks.d/*.json`) is documented.

**Merge vs override:** Cursor states (verbatim) *"All matching hooks from every source run;
when responses conflict, higher-priority sources take precedence during merge"* — so this is
a **union/merge**, not last-wins-only. Every hooks.json that matches an event contributes its
commands; conflicting *responses* to the same event are arbitrated by source priority:

> **Priority order (highest to lowest): Enterprise → Team → Project → User**

(quoted verbatim from the docs page). Note this order is the opposite of what you'd assume
from "project overrides user" in most tools — here *User* (global `~/.cursor/hooks.json`) is
the *lowest* priority, and centrally-managed (Enterprise/Team) config always wins on conflict,
though it doesn't suppress a lower-priority hook from also running.

## 3. Config schema — verbatim

Top-level shape is a **named map keyed by event name**, where each event maps to an **array**
of hook-entry objects (not a flat array of `{event, command}` records):

```json
{
  "version": 1,
  "hooks": {
    "hookName": [
      {
        "command": "string",
        "type": "command" | "prompt",
        "timeout": 10,
        "loop_limit": 5,
        "failClosed": false,
        "matcher": "string"
      }
    ]
  }
}
```

`version` is required, currently always `1`. No `$schema` key / published JSON Schema URL was
found on the docs page (checked explicitly).

Full real-world example, copied verbatim from the docs page:

```json
{
  "version": 1,
  "hooks": {
    "sessionStart": [{ "command": "./session-init.sh" }],
    "sessionEnd": [{ "command": "./audit.sh" }],
    "beforeShellExecution": [{ "command": "./hooks/audit.sh" }],
    "beforeMCPExecution": [{ "command": "./hooks/audit.sh" }],
    "afterShellExecution": [{ "command": "./hooks/audit.sh" }],
    "afterMCPExecution": [{ "command": "./hooks/audit.sh" }],
    "afterFileEdit": [{ "command": "./hooks/audit.sh" }],
    "beforeSubmitPrompt": [{ "command": "./hooks/audit.sh" }],
    "preCompact": [{ "command": "./hooks/audit.sh" }],
    "stop": [{ "command": "./hooks/audit.sh" }],
    "beforeTabFileRead": [{ "command": "./hooks/redact-secrets-tab.sh" }],
    "afterTabFileEdit": [{ "command": "./hooks/format-tab.sh" }]
  }
}
```

Per-entry fields, meanings as documented:

- **`command`** (string) — a path to a script, resolved relative to the owning hooks.json's
  root (project root for project hooks, `~/.cursor/` for user hooks).
- **`type`**: `"command"` (default, spawns a process) or **`"prompt"`** — an LLM-evaluated
  natural-language policy instead of a script. Verbatim: *"Prompt hooks use an LLM to
  evaluate a natural language condition. They're useful for policy enforcement without
  writing custom scripts."* Example from the docs:
  ```json
  { "type": "prompt", "prompt": "Does this command look safe to execute? Only allow read-only operations.", "timeout": 10 }
  ```
  This is a real deviation from the brief's "deterministic execution" framing of hooks — a
  `type: "prompt"` hook is non-deterministic (LLM-judged) by design, and cloud agents/CLI
  reportedly cannot run it at all ("prompt-based hooks require auth unavailable in cloud").
- **`timeout`** — per-script timeout; exact default value in seconds was **NOT DOCUMENTED**
  on the page (only shown set explicitly in examples, e.g. `10`).
- **`loop_limit`** (number | `null`) — *"Per-script loop limit for stop/subagentStop hooks.
  `null` means no limit. Default is `5` for Cursor hooks, `null` for Claude Code hooks."*
  Only applies to `stop`/`subagentStop` (controls how many times a `followup_message` can
  re-trigger the agent loop before Cursor stops honoring it).
- **`failClosed`** (boolean) — *"When true, hook failures (crash, timeout, invalid JSON)
  block the action instead of allowing it through. Useful for security-critical hooks."*
  Default is fail-*open* (see §8).
- **`matcher`** (string) — filter so the entry only fires for a subset of events:
  - For shell-adjacent events it matches against the **command string** as what appears to be
    a regex: examples shown are `"matcher": "curl|wget|nc"` and `"matcher": "rm|curl|wget"`.
  - For `preToolUse`/`postToolUse` it instead filters by **tool name**: *"Filter by tool
    type. Values include `Shell`, `Read`, `Write`, `Grep`, `Delete`, `Task`, and MCP tools
    using the `MCP:<tool_name>` format."* (e.g. `"matcher": "Read"`).

**Stable-identity field for a third-party installer:** **NOT DOCUMENTED / NOT FOUND.** There
is no `id`, `name`, or `description` field on a hook entry. An external tool (grim) that wants
to own, update, and idempotently remove "its" entries inside someone's hooks.json has no
first-class handle to key on — it would have to fingerprint by `command` string (e.g. always
point at a stable path like `.cursor/hooks/grim/<artifact>.sh`) and manage the surrounding
array slot itself, the same way grim already splices other vendors' native JSON/TOML files.

## 4. Event catalogue

Grouped as the brief asks; names are exactly as cased in the docs (camelCase, not
PascalCase — that's Claude Code's convention, see §11).

**Session lifecycle**
- `sessionStart` — session begins (new, resumed, etc.)
- `sessionEnd` — session ends

**Prompt submit**
- `beforeSubmitPrompt` — before the user's prompt is sent to the model

**Pre/post tool use (generic, covers built-in + MCP tools uniformly)**
- `preToolUse`
- `postToolUse`
- `postToolUseFailure`

**Shell commands specifically (also exist, alongside the generic tool hooks above)**
- `beforeShellExecution`
- `afterShellExecution`

**MCP tool usage specifically**
- `beforeMCPExecution`
- `afterMCPExecution`

**File access / edit**
- `beforeReadFile`
- `afterFileEdit`

**Subagent (Task-tool) lifecycle**
- `subagentStart`
- `subagentStop`

**Compaction**
- `preCompact`

**Stop / finish**
- `stop`

**Agent response tracking (no direct brief category; closest to "notification")**
- `afterAgentResponse`
- `afterAgentThought`

**Tab (inline completion) hooks — a separate hook family, not "Agent"**
- `beforeTabFileRead`
- `afterTabFileEdit`

**App/workspace lifecycle**
- `workspaceOpen` — fires "outside any agent session"

No dedicated `error` or `notification` event exists as such; `postToolUseFailure` is the
closest to an error hook, and there is no Claude-Code-style `Notification` event (explicitly
listed as **not supported** when importing Claude Code hooks, §11).

**Cloud-agent support matrix** (from the docs page): Supported in cloud agents —
`beforeShellExecution`, `afterShellExecution`, `beforeReadFile`, `afterFileEdit`,
`preToolUse`, `postToolUse`, `postToolUseFailure`, `subagentStart`, `subagentStop`,
`beforeSubmitPrompt`, `preCompact`, `afterAgentResponse`, `afterAgentThought`, `stop`.
**Not** supported in cloud agents: `sessionStart`/`sessionEnd` (*"Deferred; cloud agents
start read-only"*), `beforeMCPExecution`/`afterMCPExecution` (*"Deferred for read-only
environments"*), `beforeTabFileRead`/`afterTabFileEdit` (Tab is IDE-only), `workspaceOpen`
(IDE-only). Cloud agents also only run **command**-type hooks — prompt-type hooks need auth
that isn't available there.

## 5. Invocation

- **Shape:** `"command"` is a **string** (a script path), not an argv array — e.g.
  `"command": "./scripts/validate-shell.sh"`. There is no documented object/array alternative
  form; every example in the docs uses a single string.
- **Working directory:** project-scope hooks run with cwd = the **project root** (docs say
  explicitly to write `.cursor/hooks/script.sh`, not `./hooks/script.sh`, precisely because
  paths resolve from project root); user/global hooks run from `~/.cursor/`.
  Enterprise-scope cwd is **NOT DOCUMENTED**.
  When Cursor loads a **plugin's** `hooks/hooks.json`, cwd is presumably the plugin directory,
  but this was not explicitly stated on the pages fetched — **NOT DOCUMENTED**.
- **Shell used to interpret `command`:** **NOT DOCUMENTED.** The docs never state whether the
  string is run through `sh -c`/`bash -c`, or spawned directly as an executable. Community
  examples uniformly use `./script.sh` (i.e. rely on the shebang + exec bit), which is
  consistent with either model.
- **`$PATH` handling / sandboxing of the hook process itself:** **NOT DOCUMENTED** — the docs
  describe env vars *passed into* the hook (§6) but never state whether the hook inherits the
  full user shell environment/`$PATH` or a restricted one.
- **Timeout:** configurable per-entry via `"timeout"` (unit appears to be seconds, based on
  the `"timeout": 10` example vs. a `duration`-in-milliseconds field elsewhere) — but no
  documented **default** value if omitted. **NOT DOCUMENTED** (default number).
- **Concurrency / ordering:** **NOT DOCUMENTED.** No statement was found on the hooks page
  (explicitly re-checked) about whether multiple hook entries for the same event run in
  array order sequentially, or in parallel; likewise no statement on whether independent
  hooks.json sources (Enterprise/Team/Project/User) that all match one event are combined
  sequentially or concurrently. Community write-ups (gitbutler blog, [unofficial]) don't
  cover this either.
- **Blocking vs fire-and-forget:** varies by event. Events with a `permission`/`continue`
  response (`preToolUse`, `beforeShellExecution`, `beforeMCPExecution`, `beforeReadFile`,
  `beforeTabFileRead`, `subagentStart`, `beforeSubmitPrompt`) are **blocking** — per Elastic
  Security Labs' independent write-up ([unofficial] but consistent with the docs' own
  request/response framing): *"Blocking hooks pause agent execution awaiting the
  response."* Events documented as *"No output fields — fire and forget"* verbatim
  (`sessionEnd`) or with no meaningful output schema (`afterAgentThought`,
  `postToolUseFailure`) are non-blocking / observation-only.

## 6. Input payload — verbatim

Every hook receives a **JSON object on stdin**. Common base fields present on (almost) every
event, per the docs page:

```json
{
  "conversation_id": "string",
  "generation_id": "string",
  "model": "string",
  "model_id": "string",
  "model_params": [{ "id": "string", "value": "string" }],
  "hook_event_name": "string",
  "cursor_version": "string",
  "workspace_roots": ["<path>"],
  "user_email": "string | null",
  "transcript_path": "string | null"
}
```

`workspaceOpen` is the one documented exception: it *"fires outside any agent session"* and
omits `conversation_id`, `generation_id`, `model`, `session_id`, and `transcript_path`.

Per-event fields **beyond** the base (verbatim field names/types where the docs give them;
"NOT DOCUMENTED IN DETAIL" where the page doesn't spell out a shape):

| Event | Extra input fields |
|---|---|
| `sessionStart` | `session_id`, `is_background_agent`, `composer_mode` (optional). No documented `source` enum (e.g. "startup"/"resume"/"clear") — NOT DOCUMENTED. |
| `sessionEnd` | `session_id`, `reason` (`"completed"`\|`"aborted"`\|`"error"`\|`"window_close"`\|`"user_close"`), `duration_ms`, `is_background_agent`, `final_status`, `error_message` (optional) |
| `beforeSubmitPrompt` | `prompt` (text), `attachments` (array of `{type, file_path}`) |
| `preToolUse` | `tool_name`, `tool_input` (object), `tool_use_id`, `cwd`, `agent_message` (optional) |
| `postToolUse` | `tool_name`, `tool_input`, `tool_output` (JSON-stringified result), `tool_use_id`, `cwd`, `duration` (ms) |
| `postToolUseFailure` | `tool_name`, `tool_input`, `tool_use_id`, `cwd`, `error_message`, `failure_type` (`"error"`\|`"timeout"`\|`"permission_denied"`), `duration`, `is_interrupt` |
| `beforeShellExecution` | `command`, `cwd`, `sandbox` |
| `afterShellExecution` | `command`, `output`, `duration` (ms), `sandbox`. No `exit_code` field documented. |
| `beforeMCPExecution` | `tool_name`, `tool_input`, plus either `url` (URL-based MCP servers) or `command` (command-based servers) |
| `afterMCPExecution` | `tool_name`, `tool_input` (JSON string), `result_json` (JSON string of the tool response), `duration` |
| `beforeReadFile` | `file_path`, `content`, `attachments` |
| `afterFileEdit` | `file_path`, `edits: [{old_string, new_string}]` |
| `subagentStart` | `subagent_id`, `subagent_type` (e.g. `generalPurpose`, `explore`, `shell`), `task`, `parent_conversation_id`, `tool_call_id`, `subagent_model`, `is_parallel_worker`, `git_branch` (optional) |
| `subagentStop` | `subagent_type`, `status` (`"completed"`\|`"error"`\|`"aborted"`), `task`, `description`, `summary`, `duration_ms`, `message_count`, `tool_call_count`, `loop_count`, `modified_files: string[]`, `agent_transcript_path` |
| `preCompact` | `trigger` (`"auto"`\|`"manual"`), `context_usage_percent`, `context_tokens`, `context_window_size`, `message_count`, `messages_to_compact`, `is_first_compaction` |
| `stop` | NOT DOCUMENTED IN DETAIL beyond base fields |
| `afterAgentResponse` | `text` |
| `afterAgentThought` | `text`, `duration_ms` (optional) |
| `beforeTabFileRead` | `file_path`, `content` |
| `afterTabFileEdit` | `file_path`, `edits: [{old_string, new_string, range: {start_line_number, start_column, end_line_number, end_column}, old_line, new_line}]` |
| `workspaceOpen` | base fields only (minus the omissions noted above) |

No template-interpolation form (`{{file}}` in the command string) is documented anywhere —
all event data arrives via stdin JSON, never argv or string interpolation.

## 7. Output / response contract — verbatim

**Exit codes** (documented explicitly):
- **0** — success; Cursor parses stdout as the JSON response.
- **2** — *"Block the action (equivalent to `permission: "deny"`)"* — deliberately chosen to
  match Claude Code's exit-code-2 convention (§11).
- **Other non-zero** — hook treated as failed; **fails open** by default (action proceeds) —
  independently corroborated by Elastic Security Labs [unofficial]: *"Cursor also fails open
  by default: if the hook process dies without responding, the action proceeds."* Setting
  `"failClosed": true` on the entry inverts this to fail-closed.

**stdout parsing:** parsed as a **JSON response object**, not injected as free text — shape
differs per event (below). If stdout is not valid JSON: **NOT DOCUMENTED** exactly what
happens (a community source says Cursor's hooks output/debug channel surfaces malformed-JSON
diagnostics, but gives no spec for the resulting permission decision — [unofficial],
<https://blog.gitbutler.com/cursor-hooks-deep-dive>).

**stderr:** **NOT DOCUMENTED** — no statement found on where stderr goes (user-visible log,
discarded, or dev-tools-only).

**Response schema by event** (verbatim field names from the docs):

- `preToolUse`, `subagentStart`:
  ```json
  { "permission": "allow" | "deny", "user_message": "string?", "agent_message": "string?", "updated_input": "object?" }
  ```
  (`updated_input` only documented for `preToolUse`.)
- `beforeShellExecution`, `beforeMCPExecution`:
  ```json
  { "permission": "allow" | "deny" | "ask", "user_message": "string?", "agent_message": "string?" }
  ```
- `beforeReadFile`, `beforeTabFileRead`:
  ```json
  { "permission": "allow" | "deny", "user_message": "string?" }
  ```
- `postToolUse`:
  ```json
  { "updated_mcp_tool_output": "object?", "additional_context": "string?" }
  ```
- `beforeSubmitPrompt`:
  ```json
  { "continue": "boolean", "user_message": "string?" }
  ```
  Caveat: an independent practitioner (gitbutler blog, [unofficial]) reports `beforeSubmitPrompt`
  and `afterFileEdit` are, in practice, *"informational only — you cannot communicate to the
  user, agent or stop the agent with json output here,"* which conflicts with the docs'
  `continue` field. I could not resolve this discrepancy from primary sources; flagging it as
  a possible behavioral bug matching the same family as the `ask`/`allow` non-enforcement
  issue in §8, rather than asserting either side as fact.
- `subagentStop`, `stop`:
  ```json
  { "followup_message": "string?" }
  ```
  *"The `followup_message` auto-submits the next user message; default loop limit is 5,
  configurable via `loop_limit`."*
- `sessionStart`:
  ```json
  { "env": "object?", "additional_context": "string?" }
  ```
- `sessionEnd`, `afterAgentThought`, `postToolUseFailure`: **no output fields** — verbatim,
  sessionEnd is *"fire and forget."*
- `workspaceOpen`:
  ```json
  { "pluginPaths": "string[]?" }
  ```
- `afterFileEdit`, `afterShellExecution`, `afterMCPExecution`, `afterAgentResponse`,
  `afterTabFileEdit`: **NOT DOCUMENTED IN DETAIL** — treated as observational; no response
  schema spelled out (consistent with the "after"/audit framing of these events).

**Who sees the output:** `user_message` reaches the human; `agent_message` is injected back
to the model; `additional_context` is appended to model context. No field is documented as
"both" or "neither" explicitly beyond that naming convention — inferred from field names, not
a direct quote.

## 8. Reliability & limits

- **Timeout:** per-entry, configurable (`"timeout"`), default value **NOT DOCUMENTED**.
- **Non-zero exit (not 2):** fails open (action proceeds) unless `failClosed: true`.
- **Malformed JSON output:** **NOT DOCUMENTED** precisely; `failClosed: true` is described as
  covering *"crash, timeout, invalid JSON"* uniformly — so with `failClosed` unset, the
  implication is malformed JSON also fails open, though this is an inference from the
  `failClosed` description rather than a direct statement about the default path.
  the default (open) path is not spelled out verbatim for this specific case.
- **Missing binary / command not found:** **NOT DOCUMENTED**.
- **Parallel vs sequential execution, ordering guarantees:** **NOT DOCUMENTED** (checked
  explicitly, twice, against the current docs page).
- **Known, currently-open reliability bug** ([unofficial], but corroborated across multiple
  forum threads and one staff reply, so treated as more than rumor):
  - *"The `beforeShellExecution` hook's permission response (allow/ask/deny) is not respected
    by Cursor, with the allow-list taking full precedence... only 'deny' works correctly in
    all cases."* Threads: "beforeShellExecution hook permissions (allow/ask) ignored -
    allow-list takes precedence"
    (<https://forum.cursor.com/t/beforeshellexecution-hook-permissions-allow-ask-ignored-allow-list-takes-precedence/144244>),
    "Hooks: 'ask' permission broken in 2.4.21"
    (<https://forum.cursor.com/t/hooks-ask-permission-broken-in-2-4-21/150020>), and an open
    Feature Request asking Cursor to *"Support authoritative allow, deny, and ask verdicts
    from hooks"*
    (<https://forum.cursor.com/t/support-authoritative-allow-deny-and-ask-verdicts-from-hooks/161342>).
    **Practical implication for grim:** as of this research date, only the **deny** branch of
    the permission contract can be relied on in practice; `ask`/`allow` responses from a hook
    should not be assumed to change Cursor's behavior.
  - Staff-confirmed (not just a user report) gap in the CLI: on the official forum, a Cursor
    team member ("Colin") replied on 2026-03-30 to
    <https://forum.cursor.com/t/hooks-afteragentresponse-afteragentthought-not-firing-in-headless-cli/156220>
    confirming *"`afterAgentResponse` and `afterAgentThought` are not currently dispatched in
    the CLI. We're tracking this internally as a bug,"* against `cursor-agent` CLI version
    `2026.02.27-e7d2ef6` / Cursor IDE `2.6.21`. This is a genuine primary-adjacent source (a
    vendor employee, on the vendor's own forum) even though it isn't the formal docs, so I'm
    treating it as more authoritative than an ordinary forum post while still not "the docs."

## 9. Security posture

No dedicated "hooks are arbitrary code execution, be careful" warning was found **on the
hooks docs page itself** (checked explicitly — NOT FOUND). The closest official framing is
general, from Cursor's Agent Security docs (<https://cursor.com/docs/agent/security>,
fetched 2026-08-14), which never mentions hooks by name but states the general trust model
hooks sit inside:

- *"By default, terminal commands need your approval."*
- *"All MCP connections need your approval. After you approve an MCP connection, each tool
  call still needs individual approval."*
- *"Actions that could expose sensitive data require your explicit approval."*
- Workspace Trust is *"disabled by default,"* with organizations able to enforce it via MDM.

None of these sentences are about hooks specifically — hooks are a way to *automate*
responses to exactly the approval gates these sentences describe (a `beforeShellExecution`
hook can auto-answer the terminal-approval prompt), which is a meaningfully different (and
larger) trust surface than the sentences alone convey, but I did not find the docs drawing
that connection explicitly themselves.

Project-scope hooks are designed to be **shared/committed**: per search-indexed docs
phrasing, *"Project hooks are the simplest way to share hooks with your team. Place a
hooks.json file at `<project-root>/.cursor/hooks.json` and commit it to your repository. When
team members open the project in a trusted workspace, Cursor automatically loads and runs the
project hooks."* — i.e. cloning a repo with a committed `.cursor/hooks.json` is, by design,
enough for hooks to start running for teammates who open it in a trusted workspace; there is
no separate hook-specific approval dialog described beyond ordinary Workspace Trust.

Third-party security researchers (Elastic Security Labs, [unofficial] but a named, dated,
technical write-up rather than a rumor) flag the obvious consequence: *"A developer with
admin rights can remove the hooks configuration or edit the script"* — hooks are treated as
ordinary endpoint-trust-level automation, not a tamper-evident control.

Separately (and explicitly **out of this feature's scope**, per the brief — noted only to
disambiguate): there is an unrelated, actively-discussed CVE class about Cursor auto-running
**git hooks** / VS Code `tasks.json` `runOn: folderOpen` from untrusted cloned repos
(e.g. CVE-2026-26268, covered by thehackernews.com and others). That is git/task tooling, not
the Agent Hooks feature described in this document, and should not be conflated with it.

## 10. Third-party installability

**Yes — file-editing is the intended path, not UI-only.** `hooks.json` is plain JSON that
Cursor **watches and hot-reloads**: verbatim, *"Cursor watches hooks config files and reloads
them automatically."* Fallback if that doesn't work: *"If hooks still do not load, restart
Cursor."* So a `grim install` that writes/edits `.cursor/hooks.json` should take effect
without asking the user to restart the IDE, with a documented manual-restart escape hatch as
a fallback rather than the norm. No "config snapshotted at session start" gotcha was found for
hooks specifically (contrast: this pattern does exist elsewhere in the ecosystem, e.g.
Claude Code settings, but was not asserted for Cursor's hooks.json).

Because merge is additive across Enterprise/Team/Project/User sources (§2) and entries are
identified only by their `command` string (no id/name field, §3), a third-party installer
must own a private slice of the array by:
1. Writing its managed script(s) to a stable, grim-owned path (e.g.
   `.cursor/hooks/grim/<artifact-id>.sh`), and
2. Splicing hooks.json to add/remove **only** the entries whose `command` matches that owned
   path prefix, leaving all other entries (and their ordering) untouched — the same
   surgical-JSON-edit discipline grim already applies to other vendors' config files.

## 11. Trampoline viability

**This is the standout finding for Cursor.** Cursor does not just have a hooks contract of
its own — it ships a **documented compatibility shim that already reads Claude Code's native
hook config and translates it live**, via a dedicated docs page:
<https://cursor.com/docs/reference/third-party-hooks> (fetched 2026-08-14).

Verbatim: *"Cursor can load and execute hooks configured for Claude Code, allowing you to use
the same hook scripts across both tools."* Mechanics documented:

- Cursor reads `.claude/settings.local.json` → `.claude/settings.json` →
  `~/.claude/settings.json` (in that priority order) **directly**, no separate translation
  file needed from the user. This must be opted into: Settings → "Rules, Skills, Subagents" →
  *"Include third-party Plugins, Skills, and other configs"* (i.e. off by default).
- **Event name mapping** is automatic (PascalCase Claude Code name → camelCase Cursor name):
  `PreToolUse`→`preToolUse`, `PostToolUse`→`postToolUse`,
  `UserPromptSubmit`→`beforeSubmitPrompt`, `Stop`→`stop`, `SubagentStop`→`subagentStop`,
  `SessionStart`→`sessionStart`, `SessionEnd`→`sessionEnd`, `PreCompact`→`preCompact`.
  **Not supported**: Claude Code's `Notification` and `PermissionRequest` have no Cursor
  equivalent.
- **Response-shape auto-detection**: Cursor accepts *both* Claude Code's nested
  `{"hookSpecificOutput": {"permissionDecision": "deny", "updatedInput": {...}}}` shape *and*
  its own flat `{"permission": "deny", "updated_input": {...}}` shape on the same wire —
  verbatim, *"Cursor automatically maps Claude hook names to their Cursor equivalents"* and
  *"Hook scripts written for Claude Code will work in Cursor regardless of which format they
  use."*
- **Exit code 2** is honored identically in both (block the action) — Cursor's own docs frame
  this as deliberate: matching Claude Code's convention.
- Known gaps: tool-name vocabulary differs (Claude's `Bash` vs Cursor's `Shell`; Claude's
  `Glob`/`WebFetch`/`WebSearch` have no Cursor-tool equivalent at all), Cursor's
  `subagentStart` has no Claude Code counterpart, and `loop_limit`'s default differs by origin
  (`5` for native Cursor hooks, `null`/unlimited for imported Claude Code hooks) — see §3.

**What this means for grim's trampoline question, concretely:** a single generic command
(e.g. `grim hook run --client claude --event PreToolUse`) that already speaks Claude Code's
stdin/stdout contract would very plausibly run **unmodified** under Cursor too, for the
overlapping event set, *if* grim relies on Cursor's Claude-Code-import path. That is an
unusually strong result — most clients in this survey will not have this.

**However**, I would not design grim's Cursor support to depend on that import path, for
three reasons documented above: (1) it is **off by default** (a user setting toggle, not
something `grim install` can safely assume is enabled); (2) the mapping table has real gaps
(`Notification`, `PermissionRequest`, tool-name vocabulary); (3) it is Cursor reading *Claude
Code's* file format, not a stable grim-owned contract — a future Claude Code schema change
could silently break it, and grim would have no independent visibility into that. The safer
design is for grim to still write a **native** `.cursor/hooks.json` entries, using Cursor's
own camelCase event names and flat response schema (which the fetched pages confirm is a
strict superset feature-wise of what the Claude Code import path exposes) — but a grim-side
translation table between "generic portable event name" and "Cursor camelCase name" would be
nearly identical in shape to the exact table Cursor itself publishes for the Claude Code case,
so that table is effectively already vendor-validated.

Remaining blockers to a *literal* single trampoline binary (as opposed to a per-client native
config entry that all shells out to the same grim binary): none structural. `command` is a
plain string, stdin/stdout JSON is exactly the shape a small Rust binary can implement, and
there's no requirement anywhere in the docs for an in-process JS/TS module, HTTP endpoint, or
IDE-extension registration. `grim hook run --client cursor --event beforeShellExecution` as
the literal value of `"command"` in a generated hooks.json entry is directly implementable
against everything documented here.

## Sources

| URL | What it establishes | Fetched |
|---|---|---|
| https://cursor.com/docs/hooks | Primary spec: locations, schema, event catalogue, request/response shapes, priority order, plugin loop_limit note | 2026-08-14 |
| https://cursor.com/docs/reference/plugins | "Hooks" as one artifact kind inside a "Plugin" bundle; plugin-relative `hooks/hooks.json` path | 2026-08-14 |
| https://cursor.com/docs/reference/third-party-hooks | Claude Code hook import/compat shim: paths, event mapping table, response-shape auto-detection, exit-code parity, gaps | 2026-08-14 |
| https://cursor.com/changelog/1-7 | Hooks introduced in 1.7 (2025-09-29); official "still in beta" statement; named use cases | 2026-08-14 |
| https://cursor.com/docs/agent/security | General (non-hook-specific) approval/trust model hooks sit inside: terminal, MCP, workspace trust wording | 2026-08-14 |
| https://cursor.com/docs/cli/using | Confirms no hooks/hooks.json mention on this CLI page; `-p`/`--force` non-interactive permission model | 2026-08-14 |
| https://cursor.com/docs/cli/headless | Confirms no hooks/hooks.json mention on this CLI page either | 2026-08-14 |
| https://forum.cursor.com/t/beforeshellexecution-hook-permissions-allow-ask-ignored-allow-list-takes-precedence/144244 | [unofficial] `allow`/`ask` verdicts not enforced, only `deny` works | 2026-08-14 |
| https://forum.cursor.com/t/hooks-ask-permission-broken-in-2-4-21/150020 | [unofficial] same bug class, version-pinned (2.4.21) | 2026-08-14 |
| https://forum.cursor.com/t/support-authoritative-allow-deny-and-ask-verdicts-from-hooks/161342 | [unofficial] open Feature Request confirming the above is still unresolved | 2026-08-14 |
| https://forum.cursor.com/t/hooks-afteragentresponse-afteragentthought-not-firing-in-headless-cli/156220 | Staff reply (Cursor employee "Colin", 2026-03-30) confirms CLI dispatch gap as an internally-tracked bug | 2026-08-14 |
| https://blog.gitbutler.com/cursor-hooks-deep-dive | [unofficial] practitioner write-up; flags `beforeSubmitPrompt`/`afterFileEdit` as informational-only in practice (conflicts with docs' `continue` field) | 2026-08-14 |
| https://www.elastic.co/security-labs/ai-coding-agent-audit-cursor-hooks | [unofficial] independent security write-up; fail-open behavior confirmation, admin-tamper caveat | 2026-08-14 |
| https://www.infoq.com/news/2025/10/cursor-hooks/ | [unofficial] third-party news coverage corroborating 1.7 launch framing | 2026-08-14 (search snippet only, not deep-fetched) |
