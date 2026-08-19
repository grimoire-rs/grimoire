# Gemini CLI (google-gemini/gemini-cli) — hook / lifecycle-event mechanism

Research date: 2026-08-14. All sources fetched 2026-08-14 unless noted.

## 0. Headline context you need before reading the rest

Gemini CLI has a **shipped, stable, well-documented, first-class hook system** — the
richest of any client surveyed so far, closely convergent with Claude Code's hook design
(same `decision`/`reason`/`continue`/`systemMessage`/`hookSpecificOutput` vocabulary, and
it literally ships a `CLAUDE_PROJECT_DIR` compatibility alias env var).

**However**, the product itself is mid-transition. Google announced (official blog,
2026-05-19) that Gemini CLI is being superseded by a new, closed-source, Go-based
**Antigravity CLI**, and that as of **2026-06-18** (i.e. **before** today's research date)
"Gemini CLI ... will stop serving requests for Google AI Pro and Ultra, as well as those
using it free of charge." Enterprise customers on a Gemini Code Assist Standard/Enterprise
license are explicitly carved out and continue to be supported. Despite this, the
`google-gemini/gemini-cli` GitHub repo is confirmed **not archived** and **actively
developed as of today** (latest commit/nightly release timestamped 2026-08-14T01:24 UTC,
106,513 stars, 832 open issues) — see §Sources #23/#24. The maintainer announcement
(Discussion #27274) states the repo "remains available to the community as an Apache 2.0
licensed repository with no changes" and that Antigravity CLI "retains our most critical
features, including Agent Skills, **Hooks**, Subagents, and Extensions (now Antigravity
plugins)."

**Implication for grim**: the hook *mechanism* documented below is real, stable, and
fully file-based (no API calls needed to install a hook), so it remains a legitimate
target for a `grim`-materialized hook artifact. But (a) the end-user runtime that
executes hooks may itself be sunset for non-enterprise users, and (b) Antigravity CLI
— a **different binary, different config format, presumably different client id** for
grim — is where Google says the feature is headed long-term. This is a product-strategy
question for the team, not something this research can resolve; flagging it is the job.

---

## 1. Existence & name

**Exists. Name: "Hooks."** Stable and **enabled by default**.

- Origin: Epic issue **[#9070](https://github.com/google-gemini/gemini-cli/issues/9070)**,
  "Feature: Comprehensive Hooking System," opened 2025-09-22 by user `Edilmo`, labels
  `area/agent`, `area/extensions`, `priority/p2`, `workstream-rollup`. Tracked as an Epic
  with "34 of 34" sub-issues completed; closed once implementation landed.
- Landed and turned on by default in **v0.26.0** (2026-01-28). Official Google Developers
  Blog post, *"Tailor Gemini CLI to your workflow with hooks"* (2026-01-28): **"Hooks are
  enabled by default in Gemini CLI as of v0.26.0+."**
- The v0.26.0 weekly-update discussion (**[#17812](https://github.com/google-gemini/gemini-cli/discussions/17812)**,
  2026-01-28) confirms the enabled-by-default transition explicitly: **"🪝 Hooks: Now
  officially enabled by default, hooks provide a way to fully control and customize the
  agentic loop."** (The phrasing "now officially enabled by default" implies an earlier,
  opt-in/pre-GA period before v0.26.0 — exact prior version NOT DOCUMENTED in fetched
  sources.)
- No stability marker (no 🔬 "experimental" badge, no "beta" label) is attached to Hooks
  in current docs. Contrast: the docs site marks *other* current features experimental
  (Auto Memory 🔬, Git worktrees 🔬, Model steering 🔬) — Hooks carries no such marker.
- No deprecation notice for the Hooks feature itself. (See §0 for the surrounding
  product-level deprecation of Gemini CLI as a whole.)
- A master kill switch exists: `hooksConfig.enabled` (boolean, default `true`,
  **"Requires restart: Yes"**) — "Canonical toggle for the hooks system. When disabled, no
  hooks will be executed."

## 2. Config location(s)

Four config **tiers**, merged, in this exact documented precedence (highest to lowest):

1. **Project settings**: `.gemini/settings.json` in the current directory.
2. **User settings**: `~/.gemini/settings.json`.
3. **System settings**: `/etc/gemini-cli/settings.json` (Linux path; macOS/Windows admin
   paths differ — see Policy Engine section for the OS table, which documents the same
   three-OS admin split for policies; settings.md/configuration.md do not give the
   macOS/Windows equivalents for *settings* specifically — NOT DOCUMENTED for settings,
   only confirmed for policies).
4. **Extensions**: "Hooks defined by installed extensions" — via a **separate file**,
   `hooks/hooks.json`, inside each extension's directory (NOT inside the extension's
   `gemini-extension.json` manifest). Quote, `docs/extensions/reference.md`: **"Intercept
   and customize CLI behavior using hooks. Define hooks in a `hooks/hooks.json` file
   within your extension directory. Note that hooks are not defined in the
   `gemini-extension.json` manifest."** Extensions themselves live under
   `<home>/.gemini/extensions/<name>/`.

Format: **JSON** (not JSONC/TOML/YAML) for both `settings.json` and `hooks/hooks.json`.
(The *Policy Engine* — a related-but-distinct mechanism, see §disambiguation below — uses
TOML instead.)

**Env var relocation**: `GEMINI_CLI_HOME`. Verbatim (`docs/reference/configuration.md`):
> **`GEMINI_CLI_HOME`**: Specifies the root directory for Gemini CLI's user-level
> configuration and storage. By default, this is the user's system home directory. The
> CLI will create a `.gemini` folder inside this directory. ... Example:
> `export GEMINI_CLI_HOME="/path/to/user/config"`

This confirms it **replaces the home directory**, not the `.gemini` directory itself — the
`.gemini` segment is still appended by the CLI, exactly as your brief's framing stated.

**Merge semantics**: docs literally say Gemini CLI **"merges configurations from multiple
layers"** — worded as precedence for conflicting scalar values, but because each event's
hook list is an *array*, in practice hook entries compose (matcher-groups fire per event
from whichever layers define them). The exact cross-layer combination rule for **the same
event name defined at two layers simultaneously** (strict shadow vs. union of arrays) is
**NOT DOCUMENTED** verbatim beyond the word "merges" — flagged as an open question rather
than guessed.

## 3. Config schema — verbatim

**It is a named map at the top level, containing arrays of matcher-groups, each holding an
array of hook configs** — three levels of nesting, not a flat array and not a flat map.

```json
{
  "hooks": {
    "BeforeTool": [
      {
        "matcher": "write_file|replace",
        "hooks": [
          {
            "name": "security-check",
            "type": "command",
            "command": "$GEMINI_PROJECT_DIR/.gemini/hooks/security.sh",
            "timeout": 5000
          }
        ]
      }
    ]
  }
}
```
(`docs/hooks/index.md`, verbatim example.)

**Hook-matcher-group** (the object inside each event's array):

| Field | Type | Required | Description |
|---|---|---|---|
| `matcher` | `string` | No | Regex (for tool events) or exact string (for lifecycle events) to filter when the group runs. |
| `sequential` | `boolean` | No | If `true`, hooks in this group run one after another. Default (`false`) = parallel. |
| `hooks` | `array` | **Yes** | Array of hook configs. |

**Hook config** (each entry in the inner `hooks` array):

| Field | Type | Required | Description |
|---|---|---|---|
| `type` | `string` | **Yes** | Execution engine. Docs: **"Currently only `\"command\"` is supported."** (Source code reveals a second, internal-only `"runtime"` type — see §11.) |
| `command` | `string` | Yes, if `type=="command"` | The shell command to execute. |
| `name` | `string` | No | Friendly name — used in logs, in the `/hooks enable <name>`/`disable <name>` commands, in `hooksConfig.disabled`, and as half of the **stable identity key** (see below). |
| `timeout` | `number` | No | Milliseconds. Default `60000`. |
| `description` | `string` | No | Free-text purpose note. |

**Stable identity — confirmed from source, not docs.** There is **no dedicated id/uuid
field**. Identity is computed as a plain string concatenation of two free-text fields.
Verbatim, `packages/core/src/hooks/types.ts` (lines 119–125):

```typescript
/**
 * Generate a unique key for a hook configuration
 */
export function getHookKey(hook: HookConfig): string {
  const name = hook.name || '';
  const command = hook.type === HookType.Command ? hook.command : '';
  return `${name}:${command}`;
}
```

This key is what gets persisted in the **trust store** (see §9) and is implicitly what
`hooksConfig.disabled` (an array of "hook names") and the `/hooks enable|disable <name>`
CLI commands operate on. **This means a third-party installer wanting idempotent
ownership of an entry must keep BOTH `name` and `command` byte-identical across
reinstalls/updates** — see §11 for why this matters.

**Matcher syntax** (`docs/hooks/reference.md` + `docs/hooks/index.md`):
- `BeforeTool` / `AfterTool`: matcher is a **regular expression** tested against the tool
  name, e.g. `"read_.*"`, `"write_file|replace"`. MCP tools are named
  `mcp_<server_name>_<tool_name>`.
- Lifecycle events (`SessionStart`, `SessionEnd`, etc.): matcher is an **exact string**,
  e.g. `"startup"`, `"exit"`.
- Wildcard: `"*"` or `""` (empty string) matches everything.

**Extension hooks file** — confirmed identical top-level shape via source code,
`packages/cli/src/config/extension-manager.ts` (lines 1055–1073): it reads
`<extensionDir>/hooks/hooks.json`, parses it as JSON, and validates that
`rawHooks.hooks` is a non-null, non-array `object` — i.e. the extension's `hooks.json`
file has the **exact same `{"hooks": {...}}` wrapper** as `settings.json`, just in its own
file. Extensions additionally get `${extensionPath}`, `${workspacePath}`, `${/}` variable
substitution available inside command strings (`docs/extensions/reference.md`), whereas
plain `settings.json` hooks use the env vars in §6 instead.

## 4. Event catalogue — verbatim, 11 events

Full table, `docs/hooks/index.md`:

| Event | When it fires | Impact | Common use cases |
|---|---|---|---|
| `SessionStart` | Session begins (startup, resume, clear) | Inject Context | Initialize resources, load context |
| `SessionEnd` | Session ends (exit, clear) | Advisory | Clean up, save state |
| `BeforeAgent` | After user submits prompt, before planning | Block Turn / Context | Add context, validate prompts, block turns |
| `AfterAgent` | When agent loop ends | Retry / Halt | Review output, force retry or halt execution |
| `BeforeModel` | Before sending request to LLM | Block Turn / Mock | Modify prompts, swap models, mock responses |
| `AfterModel` | After receiving LLM response (**per streaming chunk**) | Block Turn / Redact | Filter/redact responses, log interactions |
| `BeforeToolSelection` | Before LLM selects tools | Filter Tools | Filter available tools, optimize selection |
| `BeforeTool` | Before a tool executes | Block Tool / Rewrite | Validate arguments, block dangerous ops |
| `AfterTool` | After a tool executes | Block Result / Context | Process results, run tests, hide results |
| `PreCompress` | Before context compression | Advisory | Save state, notify user |
| `Notification` | System notification occurs | Advisory | Forward to desktop alerts, logging |

Grouped per your brief's taxonomy:
- **Session lifecycle**: `SessionStart`, `SessionEnd`.
- **Prompt submit**: `BeforeAgent`.
- **Pre/post tool use**: `BeforeTool`, `AfterTool`.
- **Model request/response** (no bucket in your list, but real and load-bearing here):
  `BeforeModel`, `AfterModel`, `BeforeToolSelection`.
- **File edit**: **no dedicated event** — you matcher-filter `BeforeTool`/`AfterTool`
  against `write_file`/`replace`/etc. There is no `FileEdit`-specific hook.
- **Command execution**: same — matcher-filter against `run_shell_command`, no dedicated
  event.
- **Notification**: `Notification` (currently only one documented `notification_type`:
  `"ToolPermission"`).
- **Stop/finish**: `AfterAgent` (turn-level stop/retry), `SessionEnd` (process-level).
- **Compaction**: `PreCompress`.
- **Subagent**: **no dedicated hook event exists.** (The *Policy Engine*, a different
  mechanism, can target subagents by treating a subagent name as a virtual tool for
  `toolName` matching — that is not a hook.)
- **Error**: **no dedicated hook event.** `Notification`'s settings-reference description
  says it covers "notification events (errors, warnings, info)" but the event is
  Advisory/observability-only (cannot block, per §7/§8).

## 5. Invocation

- **Execution engine**: shell command string (`type: "command"`). Docs examples invoke
  `bash script.sh`, `node script.js`, `powershell script.ps1` — i.e. the `command` string
  is handed to a shell, not passed as an argv array. The exact shell binary Gemini CLI
  itself spawns (`sh -c` vs `cmd.exe /c` vs something else) is **NOT DOCUMENTED** verbatim
  in the fetched pages.
- **A second engine exists in source but is not usable externally**: `HookType.Runtime`
  (`"runtime"`) — an in-process TypeScript function reference. See §11; not reachable from
  JSON config.
- **Working directory**: the base input schema (§6) includes a `cwd` field ("Current
  working directory") delivered *to* the hook via stdin JSON. Whether the OS-level spawn
  cwd of the hook process itself is also set to that path is a reasonable inference but
  **not separately, explicitly confirmed** in the fetched docs/source excerpts.
- **`$PATH` handling**: NOT DOCUMENTED explicitly; hooks execute as shell commands so
  ordinary `$PATH` resolution presumably applies, but no doc statement was found asserting
  a sanitized/expanded `$PATH`.
- **Timeout**: default `60000` ms per hook entry; override via the `timeout` field.
- **Concurrency**: `sequential` (per matcher-group, default `false` = parallel) is the only
  documented ordering knob. Cross-group / cross-layer ordering is undocumented.
- **Blocking vs fire-and-forget**: Documented as synchronous by default. Verbatim,
  `docs/hooks/index.md`: **"Hooks run synchronously as part of the agent loop—when a hook
  event fires, Gemini CLI waits for all matching hooks to complete before continuing."**
  Explicit exceptions, quoted per-event in `docs/hooks/reference.md`:
  - `SessionEnd` — **"Best Effort: The CLI will not wait for this hook to complete and
    ignores all flow-control fields."** (true fire-and-forget)
  - `PreCompress` — **"Advisory Only. Fired asynchronously. It cannot block or modify the
    compression process."**
  - `SessionStart` — **"Advisory only: `continue` and `decision` fields are ignored.
    Startup is never blocked."** (ambiguous whether this is synchronous-but-non-blocking
    or async — docs don't disambiguate that nuance)
  - `Notification` — **"Observability Only: This hook cannot block alerts or grant
    permissions automatically. Flow-control fields are ignored."**

## 6. Input payload — verbatim

**Transport: JSON on stdin**, for every event. Universal base fields present on all
events (`docs/hooks/reference.md`):

```typescript
{
  "session_id": string,      // Unique ID for the current session
  "transcript_path": string, // Absolute path to session transcript JSON
  "cwd": string,             // Current working directory
  "hook_event_name": string, // The firing event (for example "BeforeTool")
  "timestamp": string        // ISO 8601 execution time
}
```

Per-event additional fields (all from `docs/hooks/reference.md`, verbatim field lists):

- `BeforeTool`: `tool_name` (string), `tool_input` (object, raw model-generated args),
  `mcp_context` (object, optional), `original_request_name` (string, present for tail-tool
  calls).
- `AfterTool`: `tool_name`, `tool_input` (original args), `tool_response` (object with
  `llmContent`, `returnDisplay`, optional `error`), `mcp_context`,
  `original_request_name`.
- `BeforeAgent`: `prompt` (string, the user's raw submitted text).
- `AfterAgent`: `prompt`, `prompt_response` (string, agent's final text),
  `stop_hook_active` (boolean — true if this firing is itself part of a retry chain).
- `BeforeModel` / `BeforeToolSelection`: `llm_request` (object: `model`, `messages`,
  `config`) — the "Stable Model API" shape (see below).
- `AfterModel`: `llm_request` (original) + `llm_response` (object or single streaming
  chunk).
- `SessionStart`: `source` (`"startup" | "resume" | "clear"`).
- `SessionEnd`: `reason` (`"exit" | "clear" | "logout" | "prompt_input_exit" | "other"`).
- `Notification`: `notification_type` (`"ToolPermission"` — only documented value),
  `message` (string summary), `details` (object, alert-specific metadata).
- `PreCompress`: `trigger` (`"auto" | "manual"`).

**Stable, SDK-agnostic model shapes** (`docs/hooks/reference.md`, "Stable Model API"),
verbatim:

```typescript
// LLMRequest
{
  "model": string,
  "messages": Array<{
    "role": "user" | "model" | "system",
    "content": string // Non-text parts are filtered out for hooks
  }>,
  "config": { "temperature": number, ... },
  "toolConfig": { "mode": string, "allowedFunctionNames": string[] }
}

// LLMResponse
{
  "candidates": Array<{
    "content": { "role": "model", "parts": string[] },
    "finishReason": string
  }>,
  "usageMetadata": { "totalTokenCount": number }
}
```

**Env vars** delivered to `"command"` hooks (`docs/hooks/index.md`, "Environment
variables" — described as "a sanitized environment"):
- `GEMINI_PROJECT_DIR` — absolute path to project root.
- `GEMINI_PLANS_DIR` — absolute path to the plans directory.
- `GEMINI_SESSION_ID` — current session's unique ID.
- `GEMINI_CWD` — current working directory.
- `CLAUDE_PROJECT_DIR` — **explicitly documented as an "(Alias) Provided for
  compatibility"** with Claude Code hook scripts.

**No argv-based payload. No `{{template}}` interpolation of event data into the command
string.** The only string-substitution mechanism is the *load-time* `${extensionPath}` /
`${workspacePath}` / `${/}` set, and that only applies inside extension
`gemini-extension.json` / `hooks/hooks.json` files, not to per-invocation event data.

Real payload example is not given as a single end-to-end JSON blob in the docs; the
closest is the worked script examples in `docs/hooks/writing-hooks.md`, e.g. this
`BeforeTool` consumer:
```bash
input=$(cat)
content=$(echo "$input" | jq -r '.tool_input.content // .tool_input.new_string // ""')
```

## 7. Output / response contract — verbatim

**Exit codes** (`docs/hooks/reference.md` + `docs/hooks/index.md`, both consistent):

| Exit code | Label | Behavior |
|---|---|---|
| `0` | Success | `stdout` parsed as JSON. **"Preferred code for all logic, including intentional blocks"** (e.g. `{"decision":"deny", ...}`). |
| `2` | System Block | **"Critical Block."** Target action (tool/turn/stop) is aborted; `stderr` becomes the rejection reason. |
| other | Warning | Non-fatal; **"a warning is shown, but the interaction proceeds using original parameters."** |

**Stdout discipline — "Golden Rule."** Verbatim, `docs/hooks/index.md`:
> **Silence is Mandatory**: Your script **must not** print any plain text to `stdout`
> other than the final JSON object. **Even a single `echo` or `print` call before the
> JSON will break parsing.**
> **Pollution = Failure**: If `stdout` contains non-JSON text, parsing will fail. The CLI
> will **default to "Allow"** and treat the entire output as a `systemMessage`.

That is an explicit, documented **fail-open** behavior on malformed hook output.

**stderr**: always logs/debug; also becomes the user/agent-facing message specifically on
exit-code-2 paths (e.g. becomes the tool-error `reason`, the retry feedback prompt, etc.,
per event — see §4/§8 quotes).

**Common output fields** (`docs/hooks/reference.md`):

| Field | Type | Description |
|---|---|---|
| `systemMessage` | string | Displayed immediately to the user in the terminal. |
| `suppressOutput` | boolean | If true, hides internal hook metadata from logs/telemetry. |
| `continue` | boolean | If `false`, stops the entire agent loop immediately. |
| `stopReason` | string | Shown to the user when `continue` is `false`. |
| `decision` | string | `"allow"` or `"deny"` (alias `"block"`). Impact depends on event. |
| `reason` | string | Feedback/error text when `decision` is `"deny"`. |

**Per-event `hookSpecificOutput` fields** (namespaced under `hookSpecificOutput`, which
itself always carries `hookEventName`):

- `BeforeTool.hookSpecificOutput.tool_input` — object that **merges with and overrides**
  the model's tool arguments before execution.
- `AfterTool.hookSpecificOutput.additionalContext` — text **appended** to the tool result
  sent back to the model.
- `AfterTool.hookSpecificOutput.tailToolCallRequest` — `{ name: string, args: object }`;
  chains another tool call whose result **replaces** this tool's response. Docs: "Ideal
  for programmatic tool routing."
- `BeforeAgent.hookSpecificOutput.additionalContext` — appended to the prompt, this turn
  only.
- `AfterAgent.hookSpecificOutput.clearContext` — boolean; if true, clears LLM-visible
  history while preserving the UI transcript.
- `BeforeModel.hookSpecificOutput.llm_request` — overrides parts of the outgoing request
  (e.g. swap model, change temperature).
- `BeforeModel.hookSpecificOutput.llm_response` — a **synthetic response**; if present,
  **the CLI skips the real LLM call entirely** and uses this instead.
- `BeforeToolSelection.hookSpecificOutput.toolConfig.mode` — `"AUTO"|"ANY"|"NONE"`
  (`"NONE"` "wins over other hooks" and disables all tools).
- `BeforeToolSelection.hookSpecificOutput.toolConfig.allowedFunctionNames` — whitelist;
  **union-aggregated** across multiple matching hooks. Docs explicitly note
  `BeforeToolSelection` "does **not** support `decision`, `continue`, or `systemMessage`."
- `AfterModel.hookSpecificOutput.llm_response` — replaces the model's response chunk (note:
  `AfterModel` fires **per streaming chunk**, so a replacement only affects that chunk).
- `SessionStart.hookSpecificOutput.additionalContext` — interactive: injected as the first
  turn in history; non-interactive: prepended to the user's prompt.

**Is output shown to user, model, both, neither?** Field-dependent, not a single answer:
`systemMessage` → user/terminal only. `reason` on `deny` → sent to **the agent** as a tool
error or corrective retry prompt (per `BeforeTool`/`AfterAgent` docs), and typically
surfaced to the user alongside it. `additionalContext` → injected into model-visible
context, not a direct UI element. `stopReason` → user-facing only, shown when `continue:
false`.

**Real deny/allow/context examples** (`docs/hooks/writing-hooks.md`, verbatim):
```bash
# Deny
cat <<EOF
{
  "decision": "deny",
  "reason": "Security Policy: Potential secret detected in content.",
  "systemMessage": "🔒 Security scanner blocked operation"
}
EOF
exit 0

# Allow
echo '{"decision": "allow"}'
exit 0
```
```json
{
  "hookSpecificOutput": {
    "hookEventName": "BeforeAgent",
    "additionalContext": "Recent commits:\n..."
  }
}
```

## 8. Reliability & limits

- **Default timeout**: 60000 ms per hook entry; override per-entry via `timeout`.
- **Non-zero, non-2 exit**: "Warning" tier — non-fatal, CLI proceeds with the action's
  original parameters, shows a warning to the user.
- **Malformed/non-JSON stdout**: explicit documented **fail-open to "Allow"**, with the
  raw stdout text repurposed as a `systemMessage` (see §7 "Pollution = Failure" quote).
- **Missing binary / spawn failure**: **NOT DOCUMENTED** in the fetched pages what exact
  exit/behavior class this falls into (reasonable to guess it lands in the generic
  "Warning" tier as a non-zero/spawn-error exit, but no source explicitly says so — not
  asserted as fact here).
- **Parallel vs sequential**: within one matcher-group, controlled by `sequential`
  (default `false`/parallel). Not documented across groups or across config layers.
- **Blocking**: synchronous/blocking is the default and explicitly stated
  ("Gemini CLI waits for all matching hooks to complete before continuing"), with named
  exceptions `SessionEnd` (best-effort, doesn't wait), `PreCompress` (explicitly async),
  and `SessionStart`/`Notification` (flow-control ignored, i.e. cannot stop anything even
  if they do run to completion first).

## 9. Security posture

Explicit vendor warning, `docs/hooks/index.md` (rendered as a GitHub `[!WARNING]`
callout):
> Hooks execute arbitrary code with your user privileges. By configuring hooks, you are
> allowing scripts to run shell commands on your machine.

**Three-tier trust model**, `docs/hooks/best-practices.md`:
- **System hooks** (admin-configured): **"Assumed to be the safest."**
- **User hooks** (`~/.gemini/...`): "you bear responsibility for vetting them" (paraphrase
  of fetched summary; exact sentence not independently re-quoted).
- **Project hooks** (`./.gemini/...`): **"Untrusted by default."**

**Fingerprinting / re-approval mechanic** — documented in `docs/hooks/index.md` and
verified against source:
> **Project-level hooks** are particularly risky when opening untrusted projects. Gemini
> CLI **fingerprints** project hooks. If a hook's name or command changes (for example,
> via `git pull`), it is treated as a **new, untrusted hook** and you will be warned
> before it executes.

Source confirms the mechanics precisely. `packages/core/src/hooks/trustedHooks.ts`:
`TrustedHooksManager` persists a `TrustedHooksConfig` — `{ [projectPath: string]:
string[] }` (array of trusted **hook keys**, comment: `// Array of trusted hook keys
(name:command)`) — to `<GlobalGeminiDir>/trusted_hooks.json` (i.e.
`~/.gemini/trusted_hooks.json`, or under `$GEMINI_CLI_HOME/.gemini/` if relocated). The
key itself is `getHookKey()` (quoted in full in §3) — literally `` `${name}:${command}` ``.
Hooks of `type === HookType.Runtime` are explicitly **exempted** from this trust check
(`if (hook.type === HookType.Runtime) continue;`) — consistent with that type being
internal/first-party only (§11).

**Named threat vectors**, `docs/hooks/best-practices.md`:
1. **Code execution** — "Hooks run as your user. They can do anything you can do."
2. **Data exfiltration** — malicious hooks could read prompts, code, and env vars like
   `GEMINI_API_KEY`.
3. **Prompt injection** — LLM manipulation could trigger unintended tool execution that
   in turn feeds attacker-controlled data into a hook.

**Environment variable redaction** (a general tool-execution protection, invoked by
best-practices.md as a hook-risk mitigation, but configured as a broader setting) —
`docs/reference/configuration.md`: `security.environmentVariableRedaction.enabled`
(boolean) — **default `false`** — "Enable redaction of environment variables that may
contain secrets." When on, redacts by name (`TOKEN`, `SECRET`, `PASSWORD`, `KEY`, `AUTH`,
`CREDENTIAL`, `PRIVATE`, `CERT`) and by value-pattern (private keys, certs, credentialed
URLs, known API-key formats), always-allowlists `PATH`/`HOME`/`USER`/`SHELL`/`TERM`/`LANG`
and anything prefixed `GEMINI_CLI_`. best-practices.md frames this as "strongly
recommended but currently disabled by default" for hooks specifically.

**Separately**, extensions (as a whole, not hooks specifically) get **default-on**
environment sanitization — `docs/extensions/reference.md`: "Extensions **will not**
inherit the user's full shell environment variables. They will only have access to ...
Standard safe variables (e.g., `HOME`, `PATH`, `TMPDIR`)" plus anything the manifest's
`settings[].envVar` explicitly allowlists. Whether this second, stricter, default-on
mechanism also gates *extension-sourced* `hooks/hooks.json` command execution specifically
(as opposed to just MCP servers) is not separately, explicitly re-confirmed in the fetched
excerpt — flagged rather than assumed.

## 10. Third-party installability

**Yes, realistically file-editable.** Dropping/editing `.gemini/settings.json` (project)
or `~/.gemini/settings.json` (user) with a `hooks` object requires no vendor CLI
invocation — plain JSON. This is corroborated by the docs framing hooks primarily as
something you hand-author (`mkdir -p .gemini/hooks && cat > .gemini/hooks/log-tools.sh
...`) directly into these files.

**Restart granularity is uneven, and this matters for "snapshotted at startup":**
- `hooksConfig.enabled` (the master on/off switch) is explicitly marked **"Requires
  restart: Yes"** in the settings reference.
- `hooksConfig.disabled` and `hooksConfig.notifications` carry **no such annotation** in
  the same reference table, nor do any of the eleven `hooks.<Event>` array entries
  themselves — suggesting (by the absence of the annotation the docs use everywhere else)
  that editing the hook *arrays* directly may be picked up without a restart, unlike the
  kill switch. This is an inference from a documentation convention, not an explicit
  blanket statement — flagged accordingly.
- **Extension**-sourced hooks are explicitly restart-gated: `docs/extensions/reference.md`
  — **"All management operations, including updates to slash commands, take effect only
  after you restart the CLI session."**
- Independently of restart mechanics, **project-scope hooks carry the fingerprint/trust
  gate from §9** — even if the CLI picks up an edited hook without restarting, a changed
  `name` or `command` re-triggers an interactive "untrusted hook" warning that a human
  must dismiss before the hook will actually run. For headless/CI installs this could be a
  real adoption blocker unless the trust store is also pre-seeded or the project directory
  is otherwise marked trusted.

## 11. Trampoline viability

**High — `"command"`-type hooks are about as trampoline-friendly as this survey is likely
to find.** A single generic invocation such as `grim hook run --client gemini --event
<E>` slotted into the `command` field works cleanly: uniform stdin-JSON-in /
stdout-JSON-out contract across all 11 events (differing only in which extra keys are
present), the same exit-code convention (0 = success incl. structured deny, 2 = hard
block via stderr) across every event, and a response vocabulary
(`decision`/`reason`/`continue`/`systemMessage`/`hookSpecificOutput`) that is close enough
to Claude Code's own that Gemini ships a literal `CLAUDE_PROJECT_DIR` compatibility alias
env var for hook scripts.

**Concrete blockers/friction to design around:**

1. **No dedicated id field — identity is `` `${name}:${command}` ``.** A grim-installed
   hook's stable identity is the literal concatenation of its `name` and `command`
   strings (source-confirmed, §3/§9). If grim's materialized `command` string is not
   byte-stable across `grim update` runs (e.g. it embeds a resolved absolute path that
   differs by machine, or a version-pinned wrapper path), **every update re-triggers the
   project-hook trust/fingerprint re-approval prompt** (§9) for project-scope
   installs, and could desync anything referencing the old key (e.g. a user's own
   `hooksConfig.disabled` entry naming the old identity). Design implication: grim should
   invoke a stable, version-independent trampoline path/name, and treat the hook's `name`
   field as the durable id it owns and never changes.
2. **No ownership/managed-by marker.** There's no first-class field for "grim manages
   this entry" beyond the free-text `description` field; grim's existing splice-in-place
   convention (own the member, preserve every other byte) will need a naming convention
   (e.g. a reserved `name` prefix) to reliably find-and-replace only its own entries on
   update/uninstall.
3. **Four merge layers, one of which (extensions) is a second file with the identical
   wrapper shape.** Project/user scope map cleanly onto grim's existing project/global
   scope split (→ `.gemini/settings.json` vs `~/.gemini/settings.json`). System
   (`/etc/gemini-cli/settings.json`) is admin-only and out of scope for a user-run
   installer. If grim ever ships hooks packaged as part of a Gemini **extension** artifact
   rather than spliced directly into `settings.json`, it must target the *separate*
   `<extensionDir>/hooks/hooks.json` file (confirmed shape via source, §3) — not the
   manifest.
4. **The one alternate hook engine (`"runtime"`, in-process JS function) is not reachable
   by any external installer.** Source confirms `RuntimeHookConfig` requires a live
   `HookAction` function reference (`command?: never`) that cannot be expressed in JSON —
   so there is no competing "JS module" authoring surface grim would also need to support;
   `"command"` really is the only door in for third parties. (Good news: one authoring
   surface to target, not two.)
5. **Restart/snapshot behavior is uneven and only partly documented** (§10) — grim likely
   cannot assume a freshly-installed or freshly-updated hook takes effect in an
   already-running Gemini CLI session without at least warning the user, similar to
   whatever pattern it already uses for other clients with "requires restart" settings.
6. **Matcher regex vs. exact-string duality** (tool events = regex, lifecycle events =
   exact string) is a real per-event-type branch a portable schema would need to encode
   explicitly rather than assume one matcher grammar for all events.

---

## Disambiguation — things that are NOT hooks, mentioned only for contrast

- **Policy Engine** (`docs/reference/policy-engine.md`) — a separate, TOML-based,
  *declarative* rule system (`~/.gemini/policies/*.toml`, admin paths at
  `/etc/gemini-cli/policies` (Linux) / `/Library/Application Support/GeminiCli/policies`
  (macOS) / `C:\ProgramData\gemini-cli\policies` (Windows)). Rules are `[[rule]]` blocks
  with `toolName`/`commandPrefix`/`commandRegex`/`argsPattern`/`mcpName` conditions and a
  `decision` of `allow`/`deny`/`ask_user` plus a numeric `priority`; a tiered priority
  formula (`final_priority = tier_base + (toml_priority / 1000)`, tiers Default(1) <
  Extension(2) < Workspace(3, **currently non-functional**, see
  [issue #18186](https://github.com/google-gemini/gemini-cli/issues/18186)) < User(4) <
  Admin(5)) resolves conflicts. This is **not arbitrary code execution** — it's a rule
  matcher, not a script runner — except that extensions may also ship
  `[[safety_checker]]` blocks referencing an `"in-process"` checker `type` by name (a
  fixed, presumably first-party set of built-in checkers, not user-supplied code; not
  independently verified further, out of scope). Notably, **`--yolo` / auto-approve
  configurations declared inside an *extension's* policy are explicitly ignored**: "For
  security, Gemini CLI ignores any `allow` decisions or `yolo` mode configurations in
  extension policies. This ensures that an extension cannot automatically approve tool
  calls or bypass security measures without your confirmation."
- **Approval modes / `--yolo`**: `general.defaultApprovalMode` setting
  (`"default"|"auto_edit"|"plan"`); YOLO itself is **flag-only**, not settable as a
  `general.defaultApprovalMode` value — only via `--yolo` or `--approval-mode=yolo` on the
  command line (a `security.disableYoloMode` setting can force-disable it regardless of
  flag). This interacts with the Policy Engine's per-mode rule activation (`modes =
  ["default","autoEdit","yolo"]` in a TOML rule), not with Hooks directly.
- **Extensions, skills, subagents, custom commands, MCP servers** — all real, all
  disjoint from Hooks in config location per `docs/extensions/reference.md`: custom
  commands live in `commands/*.toml`, skills in `skills/<name>/SKILL.md`, subagents (a
  **preview feature**, per an explicit `[!NOTE]` in the extensions reference) in
  `agents/*.md`, and MCP servers in the `mcpServers` map of `gemini-extension.json` itself
  — only Hooks are pushed out to the separate `hooks/hooks.json` file.

---

## Sources

| URL | What it establishes | Fetched |
|---|---|---|
| https://raw.githubusercontent.com/google-gemini/gemini-cli/main/docs/hooks/reference.md | Full I/O schema, exit codes, event-by-event input/output fields, Stable Model API shapes | 2026-08-14 |
| https://raw.githubusercontent.com/google-gemini/gemini-cli/main/docs/hooks/index.md | Overview, event table, config precedence, config schema example, env vars, security warning, fingerprinting, `/hooks` commands | 2026-08-14 |
| `docs/hooks/writing-hooks.md` (google-gemini/gemini-cli, via `gh api`) | Full tutorial with real, verbatim shell/Node.js hook scripts and matching `settings.json`; "packaging as an extension" note | 2026-08-14 |
| https://geminicli.com/docs/hooks/best-practices/ | Three-tier trust model, named threat vectors, env-redaction guidance, debugging guidance | 2026-08-14 |
| https://geminicli.com/docs/extensions/ | Confirms extensions can package "hooks" among other things (high-level only) | 2026-08-14 |
| `docs/extensions/reference.md` (google-gemini/gemini-cli, via `gh api`) | `gemini-extension.json` schema, `hooks/hooks.json` convention (hooks NOT in manifest), env-var sanitization for extensions, Policy Engine intro, variable substitution table | 2026-08-14 |
| `docs/reference/policy-engine.md` (google-gemini/gemini-cli, via `gh api`) | Full Policy Engine spec (disambiguation, not hooks): TOML schema, tiers/priority formula, approval modes, `--yolo` interaction, admin policy paths per OS | 2026-08-14 |
| `docs/reference/configuration.md` (google-gemini/gemini-cli, via `gh api`) | Canonical settings key reference: `hooksConfig.*`, `hooks.<Event>` array descriptions + defaults, `security.environmentVariableRedaction.*`, `GEMINI_CLI_HOME`, `GEMINI_CLI_TRUST_WORKSPACE`, redaction rule details | 2026-08-14 |
| `docs/cli/settings.md` (google-gemini/gemini-cli, via `gh api`) | `/settings` command, `general.defaultApprovalMode`, `security.disableYoloMode`, `security.enablePermanentToolApproval`, user/workspace settings.json paths and precedence | 2026-08-14 |
| `packages/core/src/hooks/types.ts` (google-gemini/gemini-cli source, via `gh api`) | **Primary source of truth**: `HookType` enum (`command`, `runtime`), `RuntimeHookConfig` shape (`command?: never`), `getHookKey()` identity-key function | 2026-08-14 |
| `packages/core/src/hooks/trustedHooks.ts` (google-gemini/gemini-cli source, via `gh api`) | `TrustedHooksManager`: trust-store file path (`trusted_hooks.json` under the global Gemini dir), per-project trusted-key array, `Runtime`-type exemption from trust checks | 2026-08-14 |
| `packages/cli/src/config/extension-manager.ts` (google-gemini/gemini-cli source, via `gh api`) | Confirms `hooks/hooks.json` load path and validates its `{"hooks": {...}}` wrapper shape matches `settings.json` | 2026-08-14 |
| https://github.com/google-gemini/gemini-cli/issues/9070 | Origin Epic "Feature: Comprehensive Hooking System" — opened 2025-09-22, closed, 34/34 sub-issues | 2026-08-14 |
| https://github.com/google-gemini/gemini-cli/issues/14449 | "Hook Support in Extensions" proposal (opened 2025-12-03, closed) — matches the shipped `hooks/hooks.json` convention | 2026-08-14 |
| https://github.com/google-gemini/gemini-cli/issues/15265 | "UI for Global Hooks Enable/Disable" (opened 2025-12-18, closed 2026-01-12) — maps to shipped `/hooks enable-all`/`disable-all` | 2026-08-14 |
| https://developers.googleblog.com/tailor-gemini-cli-to-your-workflow-with-hooks/ | Official launch post (2026-01-28): "Hooks are enabled by default ... as of v0.26.0+," motivating use cases | 2026-08-14 |
| https://github.com/google-gemini/gemini-cli/discussions/17812 | v0.26.0 weekly update (2026-01-28): "Now officially enabled by default" | 2026-08-14 |
| https://developers.googleblog.com/an-important-update-transitioning-gemini-cli-to-antigravity-cli/ | Official sunset announcement (2026-05-19): free/Pro tier cutoff 2026-06-18, enterprise unaffected, Antigravity CLI inherits Hooks | 2026-08-14 |
| https://github.com/google-gemini/gemini-cli/discussions/27274 | Maintainer (`LyalinDotCom`) announcement thread confirming repo stays Apache-2.0 "with no changes," enterprise support continues, feature list ported to Antigravity CLI | 2026-08-14 |
| `gh api repos/google-gemini/gemini-cli` | Repo metadata: `archived: false`, last push 2026-08-14T01:24 UTC, 106,513 stars, 832 open issues — repo is alive despite the product-transition announcement | 2026-08-14 |
| `gh api repos/google-gemini/gemini-cli/releases` | Latest release `v0.56.0-nightly.20260814...` published 2026-08-14 — active nightly cadence continues today | 2026-08-14 |
