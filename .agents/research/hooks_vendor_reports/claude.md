# Claude Code — native hook / lifecycle-event mechanism

Research date: 2026-08-14. All sources fetched same day unless noted. Client: **Claude Code**
(CLI/IDE/desktop/web agent, Anthropic). Canonical docs host redirected during this research:
`docs.claude.com/en/docs/claude-code/*` → 301 → `code.claude.com/docs/en/*`. Both are cited below;
treat `code.claude.com` as canonical going forward.

**Fidelity note on sourcing**: two fetch modes were used. (a) Raw-markdown fetches (the `.md`
suffix variant, or pages that returned obvious raw MDX source with `theme={null}` / `<Tabs>` /
`<Warning>` artifacts) are **verbatim** — quoted text below is character-for-character from the
page. (b) A few early fetches went through the WebFetch tool's summarizing pass before I
switched strategy; where a passage below is a reconstruction rather than a confirmed verbatim
quote, it is marked `[paraphrase]`. Everything else in quote blocks is verbatim.

---

## 1. Existence & name

**Exists, stable, actively developed.** Vendor calls it **"hooks"** — "user-defined shell
commands, HTTP endpoints, LLM prompts, or agents that execute automatically at specific points
in Claude Code's lifecycle." [paraphrase of opening description, hooks reference page]

Verbatim opening line of the hooks **guide** page (raw fetch):

> "Hooks are user-defined shell commands. Claude Code runs them at specific points in its
> lifecycle, which gives you deterministic control: certain actions always happen rather than
> relying on the LLM to choose to run them."
(https://code.claude.com/docs/en/hooks-guide)

No single "introduced in vX" marker exists on the hooks pages themselves. I could not establish
the original introduction version from primary sources in this session — the fetchable window
of `CHANGELOG.md` only reached back to **v2.1.199** (see §Sources); the file is presumably much
longer and lists newest-first, so the original hooks launch (which predates that window) was not
retrievable through the fetch tool's truncation. **NOT CONFIRMED.** What *is* confirmed from the
current docs is that hooks are a mature, actively-iterated surface: I found **11 distinct
version-gated behavior changes** cited inline in current docs, all in the `2.1.x` line:

| Version | What changed |
|---|---|
| v2.1.169 | `/cd` command required for directory-relocation semantics (adjacent feature, not hooks itself) |
| v2.1.191 | Comma (`,`) accepted as matcher alternator, same as `\|`; whitespace around alternatives tolerated |
| v2.1.195 | Hyphens in matcher names join the plain exact-match/list class; earlier versions evaluated hyphenated names as regex |
| v2.1.196 | `prompt_id` added to common hook input fields |
| v2.1.198 | `agent_needs_input` / `agent_completed` Notification matchers require this version+ |
| v2.1.199 | `$CLAUDE_CODE_BRIDGE_SESSION_ID` env var appears when Remote Control is connected |
| v2.1.207 | Plugin hook shell-form commands stopped substituting `${user_config.*}` (shell-injection fix) |
| v2.1.208 / v2.1.228 | `Read` deny rule also blocking `Edit`/`Write` on same path — edits gated at .208, writes at .228 |
| v2.1.210 | Prompt-hook `PreToolUse` deny semantics changed (see §7); also introduces `defer` decision |
| v2.1.211 | `.claude/settings.local.json` save location moved from starting-dir to repo root |
| v2.1.214 | Exit code 2 with a JSON payload that fails schema validation now **blocks** (previously proceeded) |
| v2.1.218 | Subagent frontmatter hooks now require the workspace-trust dialog accepted for their own folder (previously could run from an untrusted folder) |
| v2.1.229 | `disableCommandPluginSources` (managed-only) requires this version+ |

Only one hook **type** carries an explicit maturity flag: **agent hooks** (`type: "agent"`) are
labeled experimental:

> "Agent hooks are experimental. Behavior and configuration may change in future releases. For
> production workflows, prefer command hooks." (`<Warning>` block, hooks-guide, verbatim)

No deprecation notice for any hook event or field was found anywhere in the fetched pages.

---

## 2. Config location(s)

Six distinct sources, all documented in a table repeated (with matching content) on both the
hooks reference and hooks-guide pages. Verbatim table (hooks-guide, raw fetch):

| Location | Scope | Shareable |
|---|---|---|
| `~/.claude/settings.json` | All your projects | No, local to your machine |
| `.claude/settings.json` | Single project | Yes, can be committed to the repo |
| `.claude/settings.local.json` | Single project | No, gitignored when Claude Code saves a setting to it |
| Managed policy settings | Organization-wide | Yes, admin-controlled |
| Plugin `hooks/hooks.json` | When plugin is enabled | Yes, bundled with the plugin |
| Skill or agent frontmatter | While the skill or agent is active | Yes, defined in the component file |

Format: **JSON** for all settings files (no JSONC/TOML/YAML for settings.json itself). The one
exception is skill/agent **frontmatter**, which is YAML, with an embedded `hooks:` block using
the identical shape translated to YAML (see §3).

Managed-scope file paths (verbatim, from settings.md fetch):

```
macOS:   /Library/Application Support/ClaudeCode/managed-settings.json (+ managed-mcp.json, managed-settings.d/)
Linux/WSL: /etc/claude-code/managed-settings.json (+ managed-mcp.json, managed-settings.d/)
Windows (v2.1.75+): C:\Program Files\ClaudeCode\managed-settings.json (+ managed-mcp.json, managed-settings.d\)
Windows (legacy, DEPRECATED as of v2.1.75): C:\ProgramData\ClaudeCode\managed-settings.json
```
Also MDM-delivered: macOS `com.anthropic.claudecode` plist domain; Windows registry
`HKLM\SOFTWARE\Policies\ClaudeCode` (admin, highest) / `HKCU\SOFTWARE\Policies\ClaudeCode` (user,
lowest-priority policy).

**Env var relocation**: `CLAUDE_CONFIG_DIR` — sets "your configuration home" (the directory whose
`.claude` subdirectory holds settings). It is **linked from** the current permissions.md page
(`.../env-vars` anchor) as a real, current mechanism:

> "in your own configuration home, meaning your home directory or any directory whose `.claude`
> subdirectory you've set as [`CLAUDE_CONFIG_DIR`](/docs/en/env-vars)" (permissions.md, verbatim)

But I could **not** independently confirm its defining row on the env-vars page itself — that
fetch returned truncated content and reported the string absent from the visible excerpt. There
is also a closed GitHub issue about exactly this gap:

> Issue **#33430**, "[DOCS] Document CLAUDE_CONFIG_DIR environment variable for multi-account
> setups," opened 2026-03-12, **state: closed, "not planned."** Issue body (verbatim, as quoted
> back by the fetch): "The `CLAUDE_CONFIG_DIR` environment variable can be used to specify an
> alternative configuration directory instead of the default `~/.claude/`. However, this is not
> documented anywhere — not in `claude --help`, not in the official docs."
(https://github.com/anthropics/claude-code/issues/33430)

Net: `CLAUDE_CONFIG_DIR` is **real** (referenced from current official docs as of today) and
**grim already treats it as authoritative** (per this repo's own AGENTS.md env-var table), but
its own reference-page definition could not be pinned down verbatim in this session — flagging
as a minor documentation gap rather than a functional uncertainty. A second, older issue on the
same topic surfaced in search (**#25762**, "Add environment variable to configure .claude config
directory location") but I did not fetch its body — mentioned as a lead only, not verified.

**Directory-convention auto-discovery**: **No.** Every one of the six sources above is a single
named file (or a single well-known relative path inside a plugin). There is **no**
`<root>/hooks/*.json` glob-discovery convention for project/user/local scope — that pattern
exists only for **managed** settings (`managed-settings.d/*.json`, alphabetically merged, see
§Merging) and, separately, plugins get exactly one `hooks/hooks.json` (confirmed explicitly,
plugins-reference fetch): "There is **no** directory-glob auto-discovery. It is **strictly one**
`hooks/hooks.json` file per plugin."

**Merge vs. override**: Hook entries from every applicable source are **merged, not
overridden** — confirmed independently on both the hooks reference and hooks-guide pages:

> "Hook entries merge across settings levels rather than replacing: User + project + local +
> managed settings all contribute" [paraphrase, hooks reference page, first-pass fetch]

> "Multiple sources (plugin hooks and user/project settings hooks) are merged together by event
> type." (plugins-reference, paraphrase of fetch)

This is the opposite of ordinary settings keys (e.g. `model`, `outputStyle`), which follow strict
scope precedence (managed > CLI args > local > project > user) with **one winner**, per
settings.md:

> "1. **Managed** (highest) – cannot be overridden by any other scope... 2. **Command line
> arguments**... 3. **Local**... 4. **Project**... 5. **User** (lowest)"

Hooks (like permission rules) are explicitly carved out as scopes-**merge** rather than
scopes-**override**. `managed-settings.d/` drop-in files specifically: "Arrays concatenated and
de-duplicated... Objects deep-merged... sorted alphabetically... later files override scalar
values."

`disableAllHooks` is the one global kill switch, itself subject to normal precedence for
non-managed scopes (a project `false` can re-enable what a user-level `true` disabled), but
managed-set hooks are immune: "Hooks configured in managed settings still run unless
`disableAllHooks` is also set there" (hooks-guide, verbatim).

---

## 3. Config schema — verbatim

**Critical shape finding**: the hook collection is a **named map keyed by event name**, but each
event's value is an **array of "matcher groups,"** and each group's own `hooks` field is a
**second-level array of handler objects**. There is no identity field (no `id`/`name`/
`description`) anywhere in this structure — confirmed at both the settings-file level and the
plugin level (see below). This is a two-array, zero-identity schema.

Top-level shape [paraphrase, reconstructed from first-pass fetch, cross-checked against every
verbatim example seen since — the shape is confirmed correct]:

```json
{
  "hooks": {
    "EventName": [
      {
        "matcher": "string_or_regex_or_*",
        "hooks": [
          {
            "type": "command | http | mcp_tool | prompt | agent",
            "if": "permission_rule_syntax",
            "timeout": 600,
            "statusMessage": "custom_message",
            "once": false
          }
        ]
      }
    ]
  },
  "disableAllHooks": false
}
```

Real verbatim example (hooks-guide, raw fetch) showing two events as siblings inside one `hooks`
object:

```json
{
  "hooks": {
    "PostToolUse": [
      {
        "matcher": "Edit|Write",
        "hooks": [{ "type": "command", "command": "jq -r '.tool_input.file_path' | xargs npx prettier --write" }]
      }
    ],
    "Notification": [
      {
        "matcher": "",
        "hooks": [{ "type": "command", "command": "osascript -e 'display notification \"Claude Code needs your attention\" with title \"Claude Code\"'" }]
      }
    ]
  }
}
```

Skill/agent **frontmatter** (YAML) uses the identical structure, just YAML-serialized (verbatim,
hooks reference page):

```yaml
---
name: secure-operations
description: Perform operations with security checks
hooks:
  PreToolUse:
    - matcher: "Bash"
      hooks:
        - type: command
          command: "./scripts/security-check.sh"
---
```

Plugin `hooks/hooks.json` uses the exact same two-array shape (verbatim example,
plugins-reference fetch):

```json
{
  "hooks": {
    "PostToolUse": [
      {
        "matcher": "Write|Edit",
        "hooks": [
          {
            "type": "command",
            "command": "\"${CLAUDE_PLUGIN_ROOT}\"/scripts/format-code.sh"
          }
        ]
      }
    ]
  }
}
```

**Stable-identity question — answered directly**: I explicitly asked whether plugin hook entries
carry an id/name/description field. Answer, quoted from the fetch: *"Hook entries in the JSON do
**not** have `id`, `name`, or `description` fields. They are structured with: `matcher`: pattern
for matching tools; `hooks`: array of hook actions; `type`: one of `command`, `http`, `mcp_tool`,
`prompt`, or `agent`."* This holds for every scope — settings.json, settings.local.json, managed,
and plugin. The only "identity" surfaced anywhere is the **source file** a hook came from, shown
read-only in the `/hooks` TUI menu ("User Settings, Project Settings, Local Settings, Plugin
Hooks, Session Hooks, Built-in Hooks") — that is a provenance label for humans, not a
machine-addressable key a third-party installer could target for idempotent update/remove.

### Matcher syntax (verbatim table, hooks-guide)

| Matcher Value | Evaluated As |
|---|---|
| `"*"`, `""`, or omitted | Match all |
| Only letters, digits, `_`, `-`, spaces, `,`, `\|` | Exact string or list separated by `\|` or `,` |
| Contains other characters | JavaScript regex, unanchored |

Comma support and whitespace tolerance require **v2.1.191+**; hyphens joining the plain-match
class (rather than being treated as regex metacharacters) require **v2.1.195+**.

Each event type filters on a **different underlying field** — this is the full mapping, merged
from the two independent tables I pulled (hooks reference + hooks-guide; they agree):

| Event(s) | Matcher filters on |
|---|---|
| `PreToolUse`, `PostToolUse`, `PostToolUseFailure`, `PermissionRequest`, `PermissionDenied` | tool name (`Bash`, `Edit\|Write`, `mcp__.*`) |
| `SessionStart` | `startup`, `resume`, `clear`, `compact`, `fork` |
| `Setup` | `init`, `maintenance` |
| `SessionEnd` | `clear`, `resume`, `logout`, `prompt_input_exit`, `bypass_permissions_disabled`, `other` |
| `Notification` | `permission_prompt`, `idle_prompt`, `auth_success`, `elicitation_dialog`, `elicitation_url_dialog`, `elicitation_complete`, `elicitation_response`, `agent_needs_input`, `agent_completed` |
| `SubagentStart`, `SubagentStop` | agent type: `general-purpose`, `Explore`, `Plan`, custom names, or plugin-scoped `^my-plugin:reviewer$` |
| `PreCompact`, `PostCompact` | `manual`, `auto` |
| `ConfigChange` | `user_settings`, `project_settings`, `local_settings`, `policy_settings`, `skills` |
| `DirectoryAdded` | `slash_command`, `register_repo_root` |
| `StopFailure` | `rate_limit`, `overloaded`, `authentication_failed`, `oauth_org_not_allowed`, `billing_error`, `invalid_request`, `model_not_found`, `server_error`, `max_output_tokens`, `unknown` |
| `InstructionsLoaded` | `session_start`, `nested_traversal`, `path_glob_match`, `include`, `compact` |
| `Elicitation`, `ElicitationResult` | configured MCP server name |
| `FileChanged` | literal filenames (not regex), e.g. `.envrc\|.env` |
| `UserPromptExpansion` | your skill/command name |
| `UserPromptSubmit`, `PostToolBatch`, `Stop`, `TeammateIdle`, `TaskCreated`, `TaskCompleted`, `WorktreeCreate`, `WorktreeRemove`, `CwdChanged`, `MessageDisplay` | no matcher support — always fires |

MCP tool naming (verbatim pattern, both pages agree): `mcp__<server>__<tool>`, e.g.
`mcp__github__search_repositories`. Match a whole server with `mcp__<server>__.*`. Plugin-bundled
MCP servers get a scoped segment: `mcp__plugin_<plugin-name>_<server-name>__<tool>`.

### The `if` field — a second, finer filter (verbatim mechanics, hooks-guide)

`if` uses **permission-rule syntax** (`Bash(git *)`, `Edit(*.ts)`) to filter by tool name *and
arguments together*, only on `PreToolUse`, `PostToolUse`, `PostToolUseFailure`,
`PermissionRequest`, `PermissionDenied` — "Adding it to any other event prevents the hook from
running." It fails **open** (runs the hook) when the Bash command can't be parsed:

> "The filter also fails open, running your hook regardless of pattern, when the Bash command
> can't be parsed. Because the filter is best-effort, use the permission system rather than a
> hook to enforce a hard allow or deny."

Verbatim behavior table:

| `if` pattern | Bash command | Hook runs? | Why |
|---|---|---|---|
| `Bash(git *)` | `git push` | yes | command name matches |
| `Bash(git *)` | `npm test && git push` | yes | each subcommand is checked; `git push` matches |
| `Bash(git *)` | `echo $(git log)` | yes | commands inside `$()` and backticks are checked |
| `Bash(git *)` | `echo $(date)` | no | no subcommand matches `git *` |
| `Bash(git push *)` | `echo $(date)` | yes | patterns specifying more than the command name run the hook anyway on `$()`, backticks, or `$VAR` |

### Handler type fields (verbatim, hooks reference first-pass, cross-checked)

Common to all: `type` (required), `if`, `timeout`, `statusMessage`, `once` (skill frontmatter
only — "runs once per session then removed").

Timeout defaults: `command`/`http`/`mcp_tool` = 600s (10 min); `UserPromptSubmit` and
`MessageDisplay` lower this to 30s and 10s respectively; `prompt` = 30s; `agent` = 60s;
`SessionEnd` hooks of **any** type share a **1.5-second** total budget (can be raised, per-hook,
up to 60s if a longer `timeout` is set — confirmed verbatim in hooks-guide's Limitations
section).

**Command**: `command` (string), `args` (array — presence switches exec-vs-shell form, see §5),
`async`, `asyncRewake` (wakes Claude on exit code 2; implies async), `shell` (`"bash"` |
`"powershell"`, ignored when `args` set).

**HTTP**: `url` (POST target), `headers` (values support `$VAR`/`${VAR}` interpolation),
`allowedEnvVars` (allowlist — "References to unlisted variables become empty strings").

**MCP tool**: `server` (configured server name; plugin form `plugin:<plugin-name>:<server-name>`),
`tool`, `input` (object; string values support `${tool_input.path}`-style substitution). "Tool's
text content treated like command-hook stdout. If not connected or returns `isError: true`,
produces non-blocking error and continues."

**Prompt**: `prompt` (uses `$ARGUMENTS` placeholder for the hook's JSON input, verbatim: "Escape
with backslash: `\$1.00`"), `model` (defaults to a fast/Haiku-class model).

**Agent**: `prompt` (same `$ARGUMENTS` placeholder), spawns "subagent that can use tools like
Read, Grep, Glob," up to 50 tool-use turns, 60s default timeout. Experimental (see §1).

### Path placeholders

`${CLAUDE_PROJECT_DIR}`, `${CLAUDE_PLUGIN_ROOT}`, `${CLAUDE_PLUGIN_DATA}` — usable inside
command, HTTP, MCP tool, prompt, and agent hook bodies. "Prefer exec form for path placeholders.
In shell form, wrap in double quotes."

---

## 4. Event catalogue

**29 distinct event names**, confirmed by two independently-worded but content-matching tables
(hooks reference "Complete Event Table" and hooks-guide "How hooks work" table — I cross-checked
every row; they agree). Verbatim (hooks-guide raw fetch), grouped per the brief's requested
buckets:

**Session lifecycle**
- `SessionStart` — "When a session begins or resumes"
- `SessionEnd` — "When a session terminates"
- `Setup` — "When you start Claude Code with `--init-only`, or with `--init` or `--maintenance`
  in `-p` mode. For one-time preparation in CI or scripts"

**Prompt submit / expansion**
- `UserPromptSubmit` — "When you submit a prompt, before Claude processes it"
- `UserPromptExpansion` — "When a user-typed command expands into a prompt, before it reaches
  Claude. Can block the expansion"

**Pre/post tool use**
- `PreToolUse` — "Before a tool call executes. Can block it"
- `PermissionRequest` — "When a tool call needs a permission decision"
- `PermissionDenied` — "When auto mode denies a tool call, including denials without a
  classifier verdict. Use JSON `hookSpecificOutput.retry: true` to tell the model it may retry
  the denied tool call. Claude Code ignores `retry` when the classifier produced no verdict"
- `PostToolUse` — "After a tool call succeeds"
- `PostToolUseFailure` — "After a tool call fails"
- `PostToolBatch` — "After a full batch of parallel tool calls resolves, before the next model
  call"

**File edit / command execution / environment reactivity**
- `CwdChanged` — "When the working directory changes, for example when Claude executes a `cd`
  command. Useful for reactive environment management with tools like direnv"
- `FileChanged` — "When a watched file changes on disk. The `matcher` field specifies which
  filenames to watch"
- `DirectoryAdded` — "When a working directory is added mid-session via `/add-dir` or the SDK
  `register_repo_root` control request"
- `WorktreeCreate` — "When a worktree is being created via `--worktree`, `isolation: "worktree"`,
  or for a background session. Replaces default git behavior"
- `WorktreeRemove` — "When a worktree is being removed at session exit, when a subagent
  finishes, or when you delete a background session"

**Notification**
- `Notification` — "When Claude Code sends a notification" (9 sub-types via matcher, see §3)

**Message / task**
- `MessageDisplay` — "While assistant message text is displayed"
- `TaskCreated` — "When a task is being created via `TaskCreate`"
- `TaskCompleted` — "When a task is being marked as completed"

**Stop/finish**
- `Stop` — "When Claude finishes responding" — fires every turn-end, **not only task
  completion**, and **not** on user interrupts (hooks-guide Limitations, verbatim: "`Stop` hooks
  fire whenever Claude finishes responding, not only at task completion. They don't fire on user
  interrupts. API errors fire StopFailure instead")
- `StopFailure` — "When the turn ends due to an API error"

**Subagent / team**
- `SubagentStart` — "When a subagent is spawned"
- `SubagentStop` — "When a subagent finishes"
- `TeammateIdle` — "When an agent team teammate is about to go idle"

**Compaction**
- `PreCompact` — "Before context compaction"
- `PostCompact` — "After context compaction completes"

**Config / instructions**
- `ConfigChange` — "When a configuration file changes during a session"
- `InstructionsLoaded` — "When a CLAUDE.md or `.claude/rules/*.md` file is loaded into context.
  Fires at session start and when files are lazily loaded during a session"

**MCP elicitation ("error"/interaction-adjacent)**
- `Elicitation` — "When an MCP server requests user input during a tool call"
- `ElicitationResult` — "After a user responds to an MCP elicitation, before the response is
  sent back to the server"

No dedicated top-level "error" bucket exists beyond `StopFailure` and `PostToolUseFailure` (tool
error) and `PermissionDenied` (auto-mode classifier denial) — these three cover the "error"
concept the brief asked me to group.

---

## 5. Invocation

**Command hooks** — the default and only non-experimental, fully-native type — run as an actual
OS process, and the docs distinguish two invocation forms cleanly (verbatim, hooks reference
fetch, targeted re-query, high confidence):

> "**Exec form** runs when `args` is present. Claude Code resolves `command` as an executable on
> `PATH` and spawns it directly with `args` as the argument vector. There is no shell, so each
> `args` element is one argument exactly as written, and path placeholders like
> `${CLAUDE_PLUGIN_ROOT}` are substituted into `command` and into each `args` element as plain
> strings. Special characters such as apostrophes, `$`, and backticks pass through verbatim
> because there is no shell to interpret them. No shell tokenization happens on any platform."
>
> "**Shell form** runs when `args` is absent. The `command` string is passed to a shell: `sh -c`
> on macOS and Linux, Git Bash on Windows, or PowerShell when Git Bash isn't installed. Set the
> `shell` field to choose explicitly. The shell tokenizes the string, expands variables, and
> interprets pipes, `&&`, redirects, and globs."

**Working directory**: `cwd` is passed in the JSON input payload (see §6) as the working
directory *at the time the event fired* — not separately documented as a process-spawn cwd
override, implying the hook process inherits the current process cwd.

**Environment / `$PATH`**: "Hooks inherit parent environment." Claude Code injects
`CLAUDE_PROJECT_DIR`, `CLAUDE_PLUGIN_ROOT`, `CLAUDE_PLUGIN_DATA`, `CLAUDE_EFFORT`,
`CLAUDE_CODE_REMOTE` (web only), `CLAUDE_CODE_BRIDGE_SESSION_ID` (Remote Control, v2.1.199+),
`CLAUDE_PLUGIN_OPTION_<KEY>`, and strips all `OTEL_*` exporter vars from every subprocess spawn
(security-motivated — keeps telemetry config from leaking into hook-invoked tooling).
`CLAUDECODE=1` is also set (env-vars.md, verbatim) "in subprocesses Claude Code spawns (Bash and
PowerShell tools, tmux sessions, hook commands, status line commands, stdio MCP server
subprocesses)... Use to detect when a script is running inside a subprocess spawned by Claude
Code."

**HTTP hooks** are not a subprocess at all: "POST event data to a URL... The endpoint receives
the same JSON that a command hook would receive on stdin, and returns results through the HTTP
response body using the same JSON format." No shell, no PATH, no cwd — it's a plain HTTP POST
with `Content-Type: application/json`, optional interpolated `headers`.

**MCP tool hooks** invoke an already-connected MCP server's tool in-process (via the existing MCP
client connection) — no new process spawn, no shell, no stdin.

**Prompt/agent hooks** are LLM calls (single-turn / multi-turn-with-tools respectively) — not
"user code" in the shell sense at all, but the brief's scope note allows mentioning them since
they're a native part of the same mechanism.

**Concurrency / ordering**: confirmed explicitly and important for grim's design (verbatim,
hooks-guide "How hooks work" and "Combine results from multiple hooks" sections):

> "When an event fires, Claude Code runs all matching hooks in parallel."
>
> "When multiple hooks match the same event, every hook's command runs to completion before
> Claude Code merges the results. One hook returning `deny` doesn't stop sibling hooks from
> executing. Don't rely on one hook's `deny` to suppress side effects in another hook."
>
> "After all matching hooks finish, Claude Code combines their outputs. For `PreToolUse`
> permission decisions, the most restrictive answer applies, in the order `deny`, `defer`,
> `ask`, `allow`. Text from `additionalContext` is kept from every hook and passed to Claude
> together."

Ordering guarantee explicitly **absent** for conflicting mutations: "When multiple `PreToolUse`
hooks return `updatedInput` to rewrite a tool's arguments, the last one to finish takes effect.
Since hooks run in parallel, the order is non-deterministic. Avoid having more than one hook
modify the same tool's input." So: **parallel execution, no ordering guarantee, last-writer-wins
on conflicting mutation, most-restrictive-wins on conflicting permission decision.**

---

## 6. Input payload — verbatim

**Delivery mechanism**: JSON on **stdin** for command hooks; same JSON as an HTTP POST body for
HTTP hooks; JSON passed as the MCP tool's `input` (with `${...}` substitution) for MCP tool
hooks; JSON substituted wholesale into the `$ARGUMENTS` placeholder in the prompt text for
prompt/agent hooks. **No argv-based payload and no template interpolation into the command
string itself** for command hooks — argv is only ever `args` you hardcoded in config (for
exec-form), never event data. This is an important exactness point: unlike some other clients,
Claude Code does **not** support `{{file}}`-style interpolation tokens inside the `command`
string for event data — only the `${CLAUDE_PROJECT_DIR}`-style **path placeholders** (a fixed,
small, non-event-data set) interpolate into the command string itself.

**Common input fields** (every event), example straight from the docs:

```json
{
  "session_id": "abc123",
  "prompt_id": "550e8400-e29b-41d4-a716-446655440000",
  "transcript_path": "/path/to/transcript.jsonl",
  "cwd": "/current/working/dir",
  "permission_mode": "default",
  "hook_event_name": "EventName",
  "effort": { "level": "medium" },
  "agent_id": "subagent_id",
  "agent_type": "agent_name"
}
```

`permission_mode` values: `"default"`, `"plan"`, `"acceptEdits"`, `"auto"`, `"dontAsk"`,
`"bypassPermissions"`. `effort.level`: `"low"`, `"medium"`, `"high"`, `"xhigh"`, `"max"` (also
exposed as env var `$CLAUDE_EFFORT`). `prompt_id` requires **v2.1.196+** and is "absent until
first input." `agent_id`/`agent_type` only present in subagent context.

**Real example for a tool event** (`PreToolUse` on a Bash call — verbatim, hooks-guide):

```json
{
  "session_id": "abc123",
  "cwd": "/Users/sarah/myproject",
  "hook_event_name": "PreToolUse",
  "tool_name": "Bash",
  "tool_input": { "command": "npm test" }
}
```

Additional per-event fields I confirmed: `PreToolUse`/`PostToolUse` add `tool_use_id`;
`PostToolUse` adds `tool_output`; `PostToolUseFailure` adds `tool_error` in place of
`tool_output`; `Stop`/`SubagentStop` add `last_assistant_message`; `Stop` re-invocation adds
`stop_hook_active: true|false` (see the block-cap mechanism in §8); `UserPromptSubmit` adds
`prompt` (the guide calls it `prompt`, the reference's first-pass summary called it
`prompt_text` — **flagging this exact-string discrepancy rather than guessing**: the raw
hooks-guide fetch's own worked example says `UserPromptSubmit` hooks "get the `prompt` text,"
which I weight higher since it's from a verbatim raw fetch).

---

## 7. Output / response contract — verbatim

**Three channels**: exit code, stdout (parsed as plain text *or* as JSON depending on event and
content), stderr (routed differently per event — sometimes shown to Claude, sometimes to the
user only, sometimes only to a debug log, sometimes literally dropped).

### Exit codes (verbatim, hooks-guide "Read input and return output" section)

> "**Exit 0**: the hook reports no objection through its exit code. For a `PreToolUse` hook this
> doesn't approve the tool call: the normal permission flow still applies. For
> `UserPromptSubmit`, `UserPromptExpansion`, and `SessionStart` hooks, anything you write to
> stdout is added to Claude's context."
>
> "**Exit 2**: Claude Code blocks the action. Write a reason to stderr. Where it lands depends on
> the event: some events feed it to Claude as feedback so it can adjust, others show it to the
> user, and a few, such as `ConfigChange` and `Elicitation`, surface no message. Some events
> can't be blocked: for `SessionStart`, `Setup`, and others, exit 2 shows stderr to the user and
> execution continues."
>
> "**Any other exit code**: for most events, the outcome depends on what your hook printed to
> stdout: JSON that passes schema validation: Claude Code ignores the exit code, the JSON alone
> decides the outcome... JSON that parses but fails schema validation: a non-blocking error...
> No JSON on stdout: the action proceeds as a non-blocking error."

Per-event exceptions to the exit-2-blocks rule (merged from both pages; this is the closest thing
to the brief's requested "Decision control" table — **the docs do not publish one single
consolidated table with a field-path column**; I built this by cross-referencing the per-event
sections and an explicit re-query, and I flag the field-path column as **lower-confidence /
partially reconstructed**, since one targeted re-fetch returned inconsistent field paths for
several events — e.g. it first said `PostToolBatch`/`ConfigChange`/`Elicitation`/`TaskCreated`
all use `hookSpecificOutput.decision`, which conflicts with the guide's explicit statement that
`PostToolUse` and `Stop` "use a top-level `decision: "block"` field" and `PermissionRequest` uses
`hookSpecificOutput.decision.behavior`. Treat the field-path column below as **NOT FULLY
CONFIRMED** except where I have an independent verbatim quote (marked ✓):

| Event | Can block (exit 2)? | Decision field path | Confirmed how |
|---|---|---|---|
| `PreToolUse` | Yes | `hookSpecificOutput.permissionDecision` (`"allow"\|"deny"\|"ask"`, +`"defer"` in `-p` mode) | ✓ verbatim, hooks-guide |
| `PermissionRequest` | No (exit 2 not honored) | `hookSpecificOutput.decision.behavior` (`"allow"`) + optional `updatedPermissions` | ✓ verbatim, hooks-guide worked example |
| `PermissionDenied` | No | `hookSpecificOutput.retry: true` | ✓ verbatim, hooks-guide event table |
| `PostToolUse` | No (stderr shown to Claude only) | top-level `decision: "block"` | ✓ stated explicitly, hooks-guide: "`PostToolUse` and `Stop` hooks use a top-level `decision: "block"` field" |
| `PostToolUseFailure` | No | not specified beyond stderr-to-Claude | — |
| `PostToolBatch` | Yes | not independently confirmed | reconstructed only |
| `UserPromptSubmit` | Yes (blocks + erases prompt) | `hookSpecificOutput.additionalContext` for context injection; separate blocking field (see below) | ✓ verbatim for additionalContext nesting requirement |
| `UserPromptExpansion` | Yes | not independently confirmed | reconstructed only |
| `Stop` | Yes | top-level `decision: "block"` **or** top-level `continue: false` + `stopReason` | ✓ both forms independently verbatim-confirmed (see below) |
| `SubagentStop` | Yes | same as `Stop` | reconstructed by analogy |
| `ConfigChange` | Yes (except `policy_settings` source) | exit 2 or `{"decision": "block"}` per hooks-guide worked example | ✓ verbatim: "To block a change from taking effect, exit with code 2 or return `{"decision": "block"}`" |
| `Elicitation` | Yes (denies elicitation) | not independently confirmed | reconstructed only |
| `ElicitationResult` | Yes (forces decline) | not independently confirmed | reconstructed only |
| `WorktreeCreate` | Yes — **any non-zero exit fails it**, exit-code-only | none (pure exit code) | ✓ stated explicitly twice |
| `TaskCreated` / `TaskCompleted` | Yes (rollback / prevent) | not independently confirmed | reconstructed only |
| `TeammateIdle` | Yes (keeps working) | not independently confirmed | reconstructed only |
| `PreCompact` | Yes | not independently confirmed | reconstructed only |
| `SessionStart`, `Setup`, `SessionEnd`, `CwdChanged`, `DirectoryAdded`, `FileChanged`, `SubagentStart`, `PostCompact`, `WorktreeRemove`, `InstructionsLoaded`, `MessageDisplay` | **No** | stderr shown to user only (or debug-log only for `DirectoryAdded`/`WorktreeRemove`); `MessageDisplay` shows the original text regardless | ✓ stated explicitly per-event |

**Universal JSON output fields** (all hook types, all events) — verbatim example plus table from
the first-pass reference fetch, cross-checked against the guide's independent confirmation of
`continue`/`stopReason`:

```json
{
  "continue": true,
  "stopReason": "Build failed",
  "suppressOutput": false,
  "systemMessage": "Warning message",
  "terminalSequence": "\x1b]9;4;3;0\x07"
}
```

| Field | Default | Effect |
|---|---|---|
| `continue` | `true` | If `false`, **stops Claude entirely** after the hook (any hook, any event) |
| `stopReason` | none | Message shown to the user when `continue` is `false` |
| `suppressOutput` | `false` | "No effect; field accepted but ignored" [paraphrase] |
| `systemMessage` | none | Warning message shown to the user |
| `terminalSequence` | none | A terminal escape sequence — restricted to OSC 0/1/2/9/99/777 or bare BEL |

**`hookSpecificOutput` nesting is load-bearing, not cosmetic** — I confirmed this is a hard
requirement, not just a style convention, via a direct quote from the raw hooks-guide fetch:

> "For `UserPromptSubmit` hooks, use `hookSpecificOutput.additionalContext` instead to inject
> text into Claude's context. Nest `additionalContext` inside `hookSpecificOutput`; if you place
> it at the top level of the JSON, Claude Code silently ignores it."

**`updatedInput`** (PreToolUse only) rewrites the tool call's arguments before it runs — "the
last one to finish takes effect" when multiple hooks set it (non-deterministic under parallel
execution, see §5).

**Stdout/stderr routing summary** (verbatim, hooks-guide Debug section):

> "**Successful run**: you see nothing, unless the hook's JSON surfaces something, such as
> `systemMessage` or Stop hook feedback."
>
> "**Blocking error**: on most events you see the hook's feedback. When the hook's JSON made a
> blocking decision, the feedback is the reason from that decision; otherwise it is the hook's
> stderr. On a few events, such as `ConfigChange` and `Elicitation`, a block surfaces no
> message."
>
> "**Non-blocking error**: the action proceeded, and you see a `<hook name> hook error` notice
> with a short explanation, such as the first line of stderr prefixed with `Failed with
> non-blocking status code:` or a JSON validation message."

**Output size limit** (first-pass fetch, treat as paraphrase but high-confidence given it's
repeated in Limitations): plain-stdout / `additionalContext` / `systemMessage` capped at **10,000
characters**; excess is saved to a file with the transcript showing a preview + path.

**HTTP hook response handling** (paraphrase, first-pass, internally consistent with everything
else observed): 2xx + empty body = success/no-op; 2xx + JSON object body = parsed with the same
schema as command-hook stdout; 2xx + non-JSON body, non-2xx status, connection failure, or
timeout = all **non-blocking error**. Explicitly: "Unlike command hooks, HTTP hooks can't block
through status codes alone. Return 2xx with JSON body containing decision fields to block."

**Prompt/agent hook output** is a small fixed shape, not the general schema — verbatim:

> "The model's only job is to return a yes/no decision as JSON: `"ok": true`: the action
> proceeds. `"ok": false`: what happens depends on the event..."

with a `continueOnBlock: true` config flag (default false) controlling whether a `PreToolUse` /
`PostToolUse` denial reason becomes a tool error fed back to Claude (turn continues) versus ending
the turn with a warning line. Noted version-gate: "Before v2.1.210, the deny `reason` was
returned to Claude as the tool error and the turn continued" (i.e., today's default behavior is
the *opposite* of pre-2.1.210 default — a real behavioral flip, not just an addition).

---

## 8. Reliability & limits

**Timeouts**: see §3 for the default table. On timeout: "Output discarded. No decision
rendered. On `PreToolUse`: doesn't block, proceeds through normal permission flow." Separately
noted: "Agent SDK callback hooks that timeout: block the tool call on `PreToolUse`" — i.e. the
**Agent SDK's own hook mechanism behaves oppositely on timeout** from a native settings-file
command hook. This is a real, specific discrepancy worth flagging for anyone conflating "Claude
Code hooks" with "Agent SDK hooks" — they share config shape but not every runtime edge case.

**Malformed output**: JSON that parses but fails schema validation is a **non-blocking error**
(action proceeds) — except, per the v2.1.214 change noted in §1, when the exit code is
specifically **2**: "Fixed hooks with exit code 2 not blocking as documented when the hook's
stdout JSON fails schema validation" — meaning as of v2.1.214+, exit-2 blocks *regardless* of
whether the accompanying JSON is well-formed, closing what was previously a bypass.

**Missing binary / command not found**: not given a dedicated named behavior distinct from any
other nonzero-exit failure — surfaces as the generic "non-blocking error" + `<hook name> hook
error` transcript notice, with troubleshooting guidance to use absolute paths or
`${CLAUDE_PROJECT_DIR}` and to prefer exec form (`args: []`) to sidestep shell-quoting issues
entirely.

**Parallel execution**: confirmed (§5) — all matchers for one event fire concurrently, hook
processes run to completion independently, most-restrictive-wins for permission decisions,
last-write-wins (non-deterministic) for `updatedInput` conflicts.

**Blocking vs. fire-and-forget**: blocking by default (the agent loop waits for the hook to
finish or time out) *unless* `async: true` (command hooks only) — "runs in background without
blocking." A refinement, `asyncRewake: true`, still runs in the background but "wakes Claude on
exit code 2" — i.e. a deferred/async block signal, "implies `async`."

**Stop-hook runaway guard** — a concrete, load-bearing limit I confirmed verbatim (hooks-guide
Troubleshooting):

> "Claude Code overrides a Stop hook after it blocks eight times in a row without progress. Your
> hook script needs to check whether it already triggered a continuation. Parse the
> `stop_hook_active` field from the JSON input and exit early if it's `true`... If your hook
> legitimately needs more than eight iterations to converge, raise the cap with
> `CLAUDE_CODE_STOP_HOOK_BLOCK_CAP`."

**Shell-profile contamination gotcha** — worth recording verbatim since it's a real, documented
footgun for any generic trampoline that shells out: Git Bash / `BASH_ENV`-configured shells can
still source a profile that `echo`s on startup, which prepends to the hook's JSON stdout and
silently breaks JSON parsing (Claude Code then treats all of stdout as plain, non-JSON text). Fix
recommended in the docs: gate profile echoes behind `[[ $- == *i* ]]` (interactive-only check).

---

## 9. Security posture

**No dedicated "Security Considerations" heading exists on the current hooks reference or hooks
guide pages** — I searched explicitly for the words "arbitrary," "dangerous," "full permissions,"
"your permissions," "same permissions as," and a "Security"/"Security considerations" heading,
and confirmed their **absence** from both pages via two independent targeted fetches. This is
worth flagging as a genuine finding, not a gap in my research: the classic "hooks execute
arbitrary shell commands with your full user's permissions, run automatically without
confirmation" framing I expected going in is **not the current framing** as of 2026-08-14. That
warning still exists in spirit, but relocated and reworded:

**Dedicated `/docs/en/security` page**, "Additional safeguards" section (verbatim):

> "**Trust verification**: First-time codebase runs and new MCP servers require trust
> verification. Note: Trust verification is disabled when running non-interactively with the
> `-p` flag. Note: When you start Claude Code directly in your home directory, trust acceptance
> is held for the current session only and is not written to disk, so the prompt reappears on
> each launch."

Same page also carries: "Audit or block settings changes during sessions with [`ConfigChange`
hooks]" as a **team security best practice** — i.e. hooks are recommended as part of the security
tooling, not solely flagged as a risk.

**The real, load-bearing security nuance lives on `/docs/en/permissions`, "Project allow rules
and workspace trust" section — this is the single most important finding for this question**,
and I have it fully verbatim (raw fetch):

> "`permissions.allow` rules and `permissions.additionalDirectories` entries in a project's
> `.claude/settings.json` grant capability, so Claude Code applies them only after you accept the
> workspace trust dialog for that folder. The dialog lists the rules and directories the folder
> would grant so you can review them first. `deny` and `ask` rules aren't affected, since they
> only restrict."

Critically, a verbatim comparison table on that same page ("What runs before you trust a folder")
answers exactly what the brief asked ("does the client warn/prompt before running hooks from a
repo"), and the answer is **no, not by default**:

| What the repo supplies | You trusted only a parent folder | `claude -p` / SDK, folder never trusted |
|---|---|---|
| Hooks in settings files, the `env` block, helper commands (`apiKeyHelper`), a project **skill's** hooks and `allowed-tools` | **Used** | **Used.** "Workspace trust never gates a skill's `allowed-tools` in any session" |
| `permissions.allow` rules / `additionalDirectories` in `.claude/settings.json` | Not used until dialog accepted | Not used; stderr warning printed |
| Frontmatter hooks in a project **subagent**, a project `@skills-dir` plugin, `extraKnownMarketplaces` entries | Not used, no dialog offered | Not used |
| `.mcp.json` servers (incl. self-approved ones) | Asked before connecting | Connected without asking |

**In plain terms: a `.claude/settings.json` hook — the primary, most common hook location — runs
even in a `claude -p` / SDK session in a folder that has never been trusted at all**, with no
dialog, no warning, no opt-out short of `--bare`, `--setting-sources user`, or
`--settings '{"disableAllHooks": true}'`. Only **subagent frontmatter hooks** get the stronger
per-folder trust gate (and only since **v2.1.218** — before that, even subagent hooks could run
untrusted, per the same page: "Before v2.1.218, subagent frontmatter hooks could run from folders
not yet trusted by user"). Skill frontmatter hooks are explicitly **not** gated the stronger way
("Frontmatter hooks in a project skill follow the same workspace trust rule as hooks in settings
files" — hooks-guide, verbatim).

Explicit operator guidance for running an untrusted repo's `-p` session safely (verbatim,
permissions.md):

> "Before you run `claude -p` in a repository you didn't write, decide what it may run on your
> machine: Pass `--setting-sources user`... Start with `--bare`... Pass `--settings
> '{"disableAllHooks": true}'`... Add a `disabledMcpjsonServers` entry..."

**Allowlist / restriction mechanisms that do exist** (verbatim/paraphrase, settings.md +
hooks.md):
- `allowedHttpHookUrls` — "Allowlist of URL patterns HTTP hooks may target. Supports `*`
  wildcard. When set, hooks with non-matching URLs blocked. Undefined = no restrictions, empty
  array = block all."
- `allowManagedHooksOnly` (managed-settings-only) — "Blocks user, project, local, and
  non-force-enabled plugin hooks... Disables plugins with `command` source (unless
  `disableCommandPluginSources: false`)."
- `httpHookAllowedEnvVars` — env vars permitted for header interpolation in HTTP hooks.
- `strictPluginOnlyCustomization` (managed) — can lock hooks (among skills/agents/MCP) to
  plugin-or-managed sources only, as an array `["skills", "hooks"]` or blanket `true`.
- Managed settings parse **tolerantly** (v2.1.169+): "When an entry fails schema validation:
  Entry is stripped. Warning recorded. All remaining valid policies enforced. A single typo
  cannot disable entire policy." — `allowManagedHooksOnly` specifically defaults to **fail
  closed**: "Treated as `true` (restrictions apply)" if the managed value itself is malformed.

**Hooks vs. permission modes** — a second important nuance, verbatim (hooks-guide):

> "`PreToolUse` hooks fire before any permission-mode check, in every permission mode, including
> `dontAsk`. A hook that returns `permissionDecision: "deny"` blocks the tool even in
> `bypassPermissions` mode or with `--dangerously-skip-permissions`. This lets you enforce policy
> that users can't bypass by changing their permission mode. The reverse is not true: a hook
> returning `"allow"` doesn't bypass deny rules from settings."

So: hooks can **tighten** but never **loosen** past what permission rules allow — a deny-first
architecture (also true of ordinary permission rules: "Rules are evaluated in order: deny, then
ask, then allow").

---

## 10. Third-party installability

**Yes, realistically installable by editing files** — no vendor CLI or UI is required to *write*
a hook; every scope (§2) is a plain JSON (or YAML frontmatter) file on disk that any process can
edit. Confirmed indirectly by the vendor's own guidance treating direct file edits as the primary
path: "To add, modify, or remove hooks, edit your settings JSON directly or ask Claude to make
the change" (hooks-guide `<Tip>`, verbatim) — the `/hooks` menu is explicitly **read-only**: "The
`/hooks` menu is read-only. To add, modify, or remove hooks, edit your settings JSON directly."

**No native identity field** (§3) means a third-party installer cannot idempotently "own" one
array entry the way it could with a keyed map (e.g., an MCP-servers-by-name object). Practical
consequence for an installer like grim: either (a) own the *entire* `hooks` object or a *whole
event's* array (risky — collides with anything else the user or another tool adds to that same
event), or (b) encode identity out-of-band inside the `command` string itself (e.g., a
recognizable prefix/flag in the invoked command) and match on that substring to find-and-replace
its own entries on reinstall/update, leaving everything else in the array untouched. Nothing in
the docs suggests option (b) is blessed or anticipated by the vendor, but nothing forbids it
either — it is a workaround grim would be inventing, not a documented seam.

**Restart / snapshot gotcha — largely absent, with one exception**: the docs repeatedly claim
live-reload for hooks specifically. Verbatim, settings.md: "Claude Code watches your settings
files and reloads them when they change, so edits to most keys apply to the running session
without a restart. This includes `permissions`, `hooks`, and credential helpers like
`apiKeyHelper`." And hooks-guide: "If you edit settings files directly while Claude Code is
running, the file watcher normally picks up hook changes automatically" — with a fallback caveat
in Troubleshooting: "File edits are normally picked up automatically. If they haven't appeared
after a few seconds, the file watcher may have missed the change: restart your session to force
a reload." So: **hot-reload is the documented default behavior, with an admitted occasional miss**
— a third-party installer should not assume a restart is required, but should tell the user to
restart if the hook doesn't show up in `/hooks` within a few seconds. The one real snapshot-like
behavior is **`disableAllHooks` precedence resolving at the point Claude Code reads it**, not a
config-freeze — no evidence of a broader "hooks snapshotted at session start" model like some
other clients use.

**Workspace-trust interaction with installability**: per §9's table, settings-file hooks are
**not** blocked by workspace trust in most session types, which is good news for a silent
installer (the hook will actually run) but bad news for anyone hoping trust review would catch a
maliciously-installed hook — it largely won't, except in an interactive session with a truly
fresh (never-even-parent-trusted) folder.

**Skills/agents as an alternate installation surface**: grim already materializes skills and
agents for Claude Code (per this repo's own domain knowledge) — and skill/agent frontmatter can
carry a `hooks:` block scoped to "while the skill or agent is active" (§2 table) plus a
skill-only `once: true` flag for a hook that "runs once per session then removed." This is a
**second, independent installation surface** beyond settings.json splicing, with different
trust semantics (skill hooks ungated like settings hooks; subagent hooks gated, per §9) and a
different lifetime model (scoped to the component being active, not global).

---

## 11. Trampoline viability

**High viability for the `command` hook type; poor-to-moderate for the other four.**

**Why `command` is trampoline-friendly**:
- Input is always plain JSON on stdin — a single `grim hook run --client claude --event
  PreToolUse` process can `read(stdin)`, look up the real handler by event name (+ whatever
  identity grim encoded into its own invocation, per §10), and dispatch.
- Output contract is stdout JSON + exit code — fully replicable by a generic trampoline binary;
  nothing requires an in-process callback or a JS module.
- Matching (`matcher`, `if`) is done **natively by Claude Code before it ever execs the
  command** — the trampoline doesn't need to reimplement tool-name globbing or permission-rule
  parsing; it only runs when Claude Code has already decided to invoke it.
- Exec form (`args` present) sidesteps shell-quoting/escaping entirely — grim's installer can
  always emit `{"command": "grim", "args": ["hook", "run", "--client", "claude", "--event",
  "PreToolUse", "--id", "<owner-id>"]}` and never worry about shell metacharacters in its own
  invocation string.

**Concrete blockers / friction points**:
1. **No native identity field** (§3, §10) — the trampoline's own invocation string becomes the
   *only* place grim can stash an identity for idempotent update/remove. This works but is
   grim's convention, not a vendor-provided seam — a schema `id` field would be strictly better
   and doesn't exist.
2. **`hookSpecificOutput.hookEventName` must match the firing event** in the response JSON for
   several event types (confirmed pattern across every worked example in §7) — a generic
   trampoline must know, from the input's own `hook_event_name` field, which event it's replying
   to, and shape its output's nesting accordingly (`hookSpecificOutput` here, top-level
   `decision` there, `hookSpecificOutput.decision.behavior` somewhere else). This is **not a
   uniform contract** — the response shape genuinely varies per event (§7's uncertainty table),
   so a portable/generic hook schema sitting *above* Claude Code's native one would need a
   per-event mapping table baked in, not a single pass-through.
3. **Two non-command types are fundamentally not shell-command-shaped**: `http` hooks have no
   process/stdin at all (a trampoline would have to run as a persistent local HTTP listener
   instead of a spawned CLI, a materially different deployment model), and `prompt`/`agent` hooks
   invoke a Claude model directly with no shell step in between — there is no "handler" for a
   trampoline to stand in for; the LLM call *is* the handler. A portable "hook" artifact kind that
   wants to cover all five native types would need to model them as distinct handler kinds, not
   one shape.
4. **Parallel execution + non-deterministic last-write-wins on `updatedInput`** (§5, §7) means a
   trampoline that fans out to multiple installed grim-managed hooks on the same event/matcher
   needs its own internal ordering/merge logic if more than one of grim's own hooks might want to
   rewrite the same tool call — Claude Code will not arbitrate between them sensibly beyond
   "last process to exit wins."
5. **Version-sensitive response schema** (§1's version-gate table, §7's `continueOnBlock`
   default flip at v2.1.210) — a trampoline binary shipped once and reused across a user's
   Claude Code upgrades could silently change behavior underneath it as the vendor's own
   semantics shift; grim would want to pin or detect the installed Claude Code version rather
   than assume the response contract is frozen.
6. **Not a blocker, but a design gift**: the `Setup` event (`--init-only` / `--init` /
   `--maintenance` in `-p` mode) is a native "run once for CI/provisioning" hook point that maps
   almost exactly onto a package manager's own postinstall-style hook — worth reusing rather than
   reinventing if grim ever wants a Claude-Code-native provisioning step.

**Bottom line**: a `grim hook run --client claude --event <E> --id <owner-id>` trampoline
registered as a `type: "command"` handler with `args: []` (exec form) is realistic and matches
the grain of the native mechanism closely. The honest caveat is that "one generic command"
still needs **per-event knowledge of the response schema** baked into grim's own dispatcher — the
event name is uniform, the response shape is not.

---

## Sources

| URL | What it establishes | Fetched |
|---|---|---|
| https://docs.claude.com/en/docs/claude-code/hooks (→301→ https://code.claude.com/docs/en/hooks) | Hooks reference: event table, schema, matcher syntax, handler types, exit codes, `/hooks` menu, example configs (first-pass, summarized extraction) | 2026-08-14 |
| https://code.claude.com/docs/en/hooks.md | Raw-markdown re-queries: decision-control field paths (partial), CLAUDE_ vars list, exec/shell form (verbatim), version-gate list, security-term absence check | 2026-08-14 |
| https://docs.claude.com/en/docs/claude-code/hooks-guide (→301→ https://code.claude.com/docs/en/hooks-guide) | Full raw MDX dump: quickstart, all worked examples, "How hooks work," matcher table, prompt/agent hooks, HTTP hooks, Limitations, Troubleshooting, Debug — highest-fidelity source in this report | 2026-08-14 |
| https://docs.claude.com/en/docs/claude-code/settings (→301→ https://code.claude.com/docs/en/settings) | Settings file locations/paths, precedence order, merge vs. override rules, managed-settings.d/ merge mechanics, hook-related settings keys, "Available settings" key list (partial/truncated) | 2026-08-14 |
| https://code.claude.com/docs/en/permissions | **Full raw dump**: permission rule syntax, `Extend permissions with hooks`, and the load-bearing "Project allow rules and workspace trust" section incl. the trust-matrix table | 2026-08-14 |
| https://code.claude.com/docs/en/security | Full raw dump: "How we approach security," "Additional safeguards," MCP/IDE/cloud security, best practices — notably lacking any hooks-specific "arbitrary code" warning | 2026-08-14 |
| https://code.claude.com/docs/en/plugins-reference | Plugin `hooks/hooks.json` path convention, schema, confirmed absence of id/name/description fields, confirmed no directory-glob auto-discovery | 2026-08-14 |
| https://code.claude.com/docs/en/env-vars | `BASH_DEFAULT_TIMEOUT_MS`, `BASH_MAX_TIMEOUT_MS`, `CLAUDECODE` confirmed; `CLAUDE_CONFIG_DIR`/`CLAUDE_PROJECT_DIR`/`CLAUDE_ENV_FILE`/`CLAUDE_CODE_STOP_HOOK_BLOCK_CAP` not found in the fetched (truncated) excerpt | 2026-08-14 |
| https://raw.githubusercontent.com/anthropics/claude-code/main/CHANGELOG.md | Hook-related changelog entries for the fetchable window v2.1.199–v2.1.232 only; could not reach the file's oldest/original-introduction entries due to fetch-tool truncation | 2026-08-14 |
| https://github.com/anthropics/claude-code/issues/33430 | `CLAUDE_CONFIG_DIR` documentation-gap issue: opened 2026-03-12, closed "not planned," confirms the env var existed but was undocumented as of that date | 2026-08-14 |
| https://github.com/anthropics/claude-code/issues/25762 | Same-topic older issue surfaced by search ("Add environment variable to configure .claude config directory location") — **title only, body not fetched, cited as a lead not a fact** | 2026-08-14 (search only) |
| https://github.com/anthropics/claude-code/blob/main/examples/hooks/bash_command_validator_example.py | Referenced by the hooks-guide "Learn more" section as the vendor's own reference implementation of a `PreToolUse` validator hook — **linked, not independently fetched/read in this session** | not fetched |
