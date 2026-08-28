# GitHub Copilot — hook / lifecycle-event mechanism

Research date: 2026-08-14. Client: **copilot** (GitHub Copilot), covering three surfaces:

1. Standalone **Copilot CLI** (`~/.copilot`, `COPILOT_HOME`)
2. VS Code's **embedded Copilot agent mode**
3. The cloud **Copilot coding agent** ("cloud agent")

## Top-line surprise

Contrary to the brief's working assumption ("cloud-only setup steps are NOT hooks"),
**GitHub Copilot has a real, mature, actively-developed native hook system**, and it covers
**all three surfaces** — not just "setup steps" for the cloud agent. GitHub's own reference
doc states outright:

> "Hooks are supported in two Copilot surfaces: Copilot CLI and Copilot cloud agent. Most of
> the configuration format and event payloads are identical, but the execution environment
> and the set of events that can fire differ."
> — <https://docs.github.com/en/copilot/reference/hooks-reference> (fetched 2026-08-14)

VS Code's embedded agent mode has its **own, separately-documented** hook implementation
(same `.github/hooks/*.json` directory, overlapping but not identical event set), explicitly
built to the same wire format:

> "Agent hooks (Preview): Hooks let you execute custom shell commands at key lifecycle points
> during agent sessions." ... "The feature uses the same format as Claude Code and Copilot CLI,
> allowing configuration reuse across these tools."
> — VS Code 1.109.3 update notes, <https://code.visualstudio.com/updates/v1_109> (fetched 2026-08-14)

So all three surfaces are covered by ONE research doc because they share one underlying idea
(GitHub calls it "Hooks" everywhere) — but the three implementations differ in event coverage,
execution environment, and maturity, so each is answered separately below.

---

## Surface 1 — Copilot CLI (`~/.copilot`, `COPILOT_HOME`)

### 1. Existence & name

Exists. Vendor name: **"Hooks"**. **Not** flagged preview/beta/experimental anywhere in the
official reference or how-to docs (no banner, no "(Preview)" marker — contrast with VS Code,
below). Introduced early and iterated continuously; still evolving weekly as of August 2026.

**Version history (from `github/copilot-cli`'s own `changelog.md`,
<https://github.com/github/copilot-cli/blob/main/changelog.md>, fetched via GitHub API
2026-08-14 — dates below are the changelog's own version-date headers):**

| Version | Date | What landed |
|---|---|---|
| 0.0.396 | 2026-01-27 | First hook: `preToolUse` can deny tool execution and modify arguments |
| 0.0.401 | 2026-02-03 | `agentStop`, `subagentStop` hooks |
| 0.0.402 | 2026-02-03 | Plugins can provide hooks for session lifecycle events |
| 0.0.422 | 2026-03-05 | Personal hooks from `~/.copilot/hooks`; startup prompt hooks |
| 1.0.2 | 2026-03-06 | `command` field as cross-platform alias for `bash`/`powershell`; `timeout` alias for `timeoutSec` |
| 1.0.4 | 2026-03-11 | `disableAllHooks` flag; `ask` permission decision |
| 1.0.5 | 2026-03-13 | `preCompact` hook; hook files without a `version` field now accepted |
| 1.0.6 | 2026-03-16 | **Cross-client compatibility**: PascalCase event names accepted (VS Code/Claude Code style) alongside camelCase; Claude Code's nested matcher/hooks structure supported; "Open Plugins spec" compatibility (`.lsp.json`, PascalCase, `exclusive` path mode, `:` namespace separator) |
| 1.0.7 | 2026-03-17 | `subagentStart` hook |
| 1.0.8 | 2026-03-18 | Hooks definable inline in `settings.json`/`settings.local.json`/`config.json`; **security fix**: "Repo-level hooks are loaded only after folder trust is confirmed, not before the trust dialog is shown" |
| 1.0.15 | 2026-04-01 | `postToolUseFailure` hook added; `postToolUse` now fires only after successful tool calls |
| 1.0.16 | 2026-04-02 | `permissionRequest` hook |
| 1.0.19 | 2026-04-06 | `notification` hook (fires on shell completion, permission prompts, elicitation dialogs, agent completion/idle) |
| 1.0.21 | 2026-04-07 | PascalCase-named hooks receive VS Code-compatible **snake_case** payloads (`hook_event_name`, `session_id`, ISO-8601 timestamps) |
| 1.0.26 | 2026-04-14 | **HTTP hook type** — POST JSON to a URL instead of running a local command |
| 1.0.40 | 2026-05-01 | `-p` (prompt/non-interactive mode) gates repo hooks and workspace MCP behind opt-in env vars `GITHUB_COPILOT_PROMPT_MODE_REPO_HOOKS` / `GITHUB_COPILOT_PROMPT_MODE_WORKSPACE_MCP`, "for secure-by-default behavior" |
| 1.0.51 | 2026-05-20 | `preMcpToolCall` hook (controls outgoing MCP request metadata) |
| 1.0.55 | 2026-05-28 | `preToolUse` hook **errors now deny** the tool call instead of silently allowing (fail-closed hardening) |
| 1.0.76 | 2026-07-29 | Hook output bounded at **10 MiB** per invocation (DoS/memory-exhaustion hardening) |
| 1.0.78 | 2026-08-03 | Enable/disable UI for hooks (among plugins/instructions/agents/LSP) in `/plugins` |
| 1.0.80 | 2026-08-14 | current version at time of research |

CLI reached GA on 2026-02-25 (<https://github.blog/changelog/2026-02-25-github-copilot-cli-is-now-generally-available/>)
— i.e., hooks predate CLI's own GA by a month and have shipped roughly 40+ hook-related
changelog entries since, including live bug reports still open today (see §8).

### 2. Config location(s)

Full priority/merge order per the reference doc (**all layers merge — see below**):

1. **Policy-level** (enterprise-managed, CLI only): `/etc/github-copilot/policy.d/*.json`
   (Linux/macOS) or `C:\ProgramData\GitHub\Copilot\policy.d\*.json` (Windows). Cannot be
   disabled by `disableAllHooks`; on POSIX must be root-owned and not group/world-writable.
2. **Repository-level**: `.github/hooks/*.json` (any filename, directory glob).
3. **User-level**: `~/.copilot/hooks/` (or `$COPILOT_HOME/hooks/` if `COPILOT_HOME` is set).
4. **Inline, repository**: `hooks` key inside `.github/copilot/settings.json` or
   `.github/copilot/settings.local.json` (the latter meant to be gitignored).
5. **Inline, user**: `hooks` key inside `~/.copilot/settings.json`.
6. **Plugin hooks**: `hooks.json` (or `hooks/hooks.json`) inside a plugin's own installation
   directory (declared by the plugin itself).

Format: **JSON** (JSONC-ish in `settings.json`, which supports comments per the config-dir
reference). `COPILOT_HOME` relocates the entire `~/.copilot` tree (confirmed at
<https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference>,
fetched 2026-08-14); `COPILOT_CACHE_HOME` overrides only the cache subtree.

**Merge behavior — explicit and important:** "Hooks are loaded from multiple sources and
combined. When the same event appears in multiple sources, all hook entries from all sources
are run." This is **additive union**, not override/last-wins.

**Reload timing:** the CLI's own how-to doc states plainly: "Changes to hook configurations
are loaded when the CLI starts" (`[!NOTE]` callout at
<https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/use-hooks>, fetched
2026-08-14) — i.e., **no live reload**; a newly-installed hook file needs a fresh CLI session
to take effect. (Contrast VS Code, which reloads live — see Surface 2 §10.)

### 3. Config schema — verbatim

Top level is a **named map keyed by event name**, whose values are **arrays** of hook entries
(not a flat array of `{event, ...}` objects):

```json
{
  "version": 1,
  "disableAllHooks": false,
  "hooks": {
    "preToolUse": [
      {
        "type": "command",
        "bash": "YOUR_BASH_COMMAND",
        "powershell": "YOUR_POWERSHELL_COMMAND",
        "cwd": "OPTIONAL/WORKING/DIRECTORY",
        "env": { "VAR": "VALUE" },
        "timeoutSec": 30
      }
    ]
  }
}
```

Three hook entry **types**:

- **`command`** (default when `type` omitted): one of `bash` / `powershell` / `command`
  (cross-platform alias, added v1.0.2) is required; optional `cwd`, `env`, `timeoutSec`
  (alias `timeout`), `matcher`.
- **`http`** (added v1.0.26): requires `type: "http"` and `url`; optional `headers`,
  `allowedEnvVars`, `timeoutSec`, `matcher`. Only `https://` allowed by default; plain
  `http://` rejected except localhost, and only when `COPILOT_HOOK_ALLOW_LOCALHOST=1` is set.
- **`prompt`**: requires `type: "prompt"` and `prompt` (a string injected/handled without
  shelling out).

**Matcher syntax:** an optional regex, compiled as `^(?:PATTERN)$` (i.e., **full match**, not
substring/glob) against: `toolName` (`preToolUse`/`postToolUse`/`permissionRequest`),
`notification_type` (`notification`), `trigger` (`preCompact`, values `"manual"`/`"auto"`), or
`agentName` (`subagentStart`). PascalCase-named hooks (`PreToolUse`, `PermissionRequest`) get
**Claude-format matcher semantics** instead: `*`, `**`, or empty string match all; literal
names or `|`-alternations match tokens; anything else is case-sensitive regex. Two parallel
matcher dialects live side by side, keyed off which event-name casing you use.

**Stable identity for a third-party installer — does NOT exist.** No hook entry (`command`,
`http`, or `prompt`) carries an `id`, `name`, or `description` field anywhere in the schema.
An external installer cannot own, update, or remove *one entry* inside a shared array
idempotently by ID — it can only safely own an **entire file** it wrote itself (e.g., a
dedicated `.github/hooks/grimoire-<name>.json`) and replace/delete that file wholesale. This
matches the CLI directory-glob design (any `.json` file under `.github/hooks/` is picked up)
and is the realistic integration seam for a third-party tool.

`"disableAllHooks": true` at the top of any one file disables all hooks *sourced from that
file* — in repo `settings.json` this suppresses every non-policy source; it cannot suppress
policy-level (enterprise) hooks.

### 4. Event catalogue (verbatim names, CLI)

Both a camelCase and (as of 1.0.6/1.0.21) an accepted PascalCase alias exist for most events:

| Event (camelCase / PascalCase) | Fires |
|---|---|
| `sessionStart` / `SessionStart` | New session begins or a previous session is resumed |
| `sessionEnd` / `SessionEnd` | Session completes or is terminated |
| `userPromptSubmitted` / `UserPromptSubmit` | User submits a prompt (can short-circuit the LLM entirely as of 1.0.44) |
| `userPromptTransformed` | After prompt transformation/expansion, before the model sees it |
| `preToolUse` / `PreToolUse` | Before any tool call — the only event with real access-control power |
| `postToolUse` / `PostToolUse` | After a **successful** tool call (split from failures at 1.0.15) |
| `postToolUseFailure` / `PostToolUseFailure` | After a tool call that failed (added 1.0.15) |
| `preMcpToolCall` | Before an outgoing MCP tool call (added 1.0.51; controls request metadata) |
| `preCompact` / `PreCompact` | Before context compaction; matcher on `trigger` (`manual`/`auto`) |
| `subagentStart` | A subagent is spawned; matcher on `agentName`; can inject context into the subagent prompt |
| `subagentStop` / `SubagentStop` | A subagent finishes |
| `agentStop` / `Stop` | The whole agent turn ends |
| `errorOccurred` / `ErrorOccurred` | An error occurs |
| `notification` | Async system notifications: `notification_type` ∈ `shell_completed`, `shell_detached_completed`, `agent_completed`, `agent_idle`, `permission_prompt`, `elicitation_dialog`. **Does not fire under cloud agent.** |
| `permissionRequest` / `PermissionRequest` | Before the built-in permission service (rules engine/session approvals/auto-allow-deny/user prompt) runs; lets a script pre-empt the prompt. **Does not apply under cloud agent** (tool calls there are pre-approved, no interactive user). |

### 5. Invocation

`command`-type hooks run as a **child process** of the CLI on the developer's machine, in the
`cwd` given (default: project root; `LSP`/hook-matching keep this convention) — a real shell
command string, not an argv array (separate `bash` and `powershell` strings, unified later by
the generic `command` alias). `http`-type hooks POST JSON to a URL instead of shelling out —
no local process at all. `prompt`-type hooks inject a literal string, no subprocess.

Working directory: project root by default, or the hook's own `cwd`. Timeout: `timeoutSec`
(default **30s**), overridable per hook. Ordering/concurrency: **hooks for the same event run
sequentially, in array order** — if one denies (or errors on `preToolUse`), later ones in the
same event array do not need to (and per community reports, do not) get to override that.
**Known reliability gap** (open as of fetch): under *parallel* tool calls, `preToolUse` hooks
are dispatched "one-by-one with gaps" rather than truly in parallel, which one filed bug
describes as a silent-bypass risk under timeout — see
<https://github.com/github/copilot-cli/issues/2893> ("preToolUse hooks silently bypassed under
parallel tool calls (timeout->allow fallback + serial dispatch)", state **OPEN**, opened
2026-04-22, last updated 2026-06-19; labels `area:permissions`, `area:plugins`) [primary
source: GitHub issue tracker].

Plugin-provided hooks receive `PLUGIN_ROOT`, `COPILOT_PLUGIN_ROOT`, and (for Claude-format
compat) `CLAUDE_PLUGIN_ROOT` env vars pointing at the plugin's install dir (added 1.0.24), plus
`{{project_dir}}`/`{{plugin_data_dir}}` template interpolation in the command string (1.0.12).
`$PATH`: inherited from the CLI's own process environment (no isolation documented).

### 6. Input payload — verbatim

JSON on **stdin**. Two shapes exist depending on the event-name casing used in your config:

**camelCase style** (native CLI shape), `sessionStart` example:

```typescript
{
    sessionId: string;
    timestamp: number;      // Unix ms
    cwd: string;
    source: "startup" | "resume" | "new";
    initialPrompt?: string;
}
```

`preToolUse`:

```typescript
{ sessionId: string; timestamp: number; cwd: string; toolName: string; toolArgs: unknown; }
```

`postToolUse` adds `toolResult: { resultType: "success"; textResultForLlm: string }`.

`agentStop`/`Stop`:

```typescript
{ sessionId: string; timestamp: number; cwd: string; transcriptPath: string;
  stopReason: "end_turn"; stop_hook_active: boolean; }
```

(`stop_hook_active` lets a hook detect it's being re-invoked after a forced continuation —
see the runaway-loop guard in §8.)

**PascalCase/"VS Code-compatible" style** (since 1.0.21, when the event name in your config
uses PascalCase): snake_case field names, plus `hook_event_name: "<EventName>"`,
`session_id`, ISO-8601 `timestamp` — i.e., the payload shape *itself* switches to match
whichever casing convention you registered the hook under. This is a deliberate
interoperability shim, not an accident: 1.0.6's changelog line is explicit about "Hook
configuration files now work across VS Code, Claude Code, and the CLI without modification."

### 7. Output / response contract — verbatim

Exit codes (`command` hooks):

| Code | Meaning |
|---|---|
| `0` | Success; stdout parsed as the hook's JSON output |
| `2` | Historically "warning"; for `preToolUse` and `permissionRequest` specifically, **treated as deny** (hardened at 1.0.70: "preToolUse hooks exit with code 2 deny tool calls") |
| other non-zero | **Fail-open** (logged, execution continues) — **except** `preToolUse`, which is **fail-closed**: a non-zero/errored `preToolUse` hook denies the tool call even if its stdout JSON said `"permissionDecision":"allow"` (hardened at 1.0.55: "preToolUse hooks now respect... additionalContext fields" / "preToolUse hook errors now deny the tool call instead of silently allowing execution") |
| timeout | **Fail-open** for all events (warning logged); confirmed at 1.0.67: "Allow tool calls to continue when hooks time out" |

Response JSON (stdout), by event:

- **`preToolUse`**: `{ "permissionDecision": "allow"|"deny"|"ask", "permissionDecisionReason": "string", "modifiedArgs": {} }` — `permissionDecisionReason` required when denying; `"allow"` suppresses the normal interactive tool-approval prompt (1.0.18).
- **`postToolUse`**: `{ "modifiedResult": { "resultType": "success", "textResultForLlm": "string" }, "additionalContext": "string" }` — `additionalContext` is injected as a system message to the model (only as of 1.0.49/1.0.100-ish hardening; earlier versions silently discarded it).
- **`agentStop` / `subagentStop`**: `{ "decision": "block"|"allow", "reason": "string", "modifiedResponse": "string" }` — `modifiedResponse` applies only to `subagentStop`; `reason` required for `"block"`. **Runaway guard**: after **8 consecutive** `"block"` continuations, the CLI forces the turn to end regardless (uses the `stop_hook_active` flag so a hook can self-limit).
- **`notification`**: `{ "additionalContext"?: string }` — injected as a user message if present.
- **`permissionRequest`**: `{ "behavior": "allow"|"deny", "message"?: string, "interrupt"?: boolean }`.
- **Common fields** available broadly (VS Code-compatible shape): `continue` (bool; `false` stops the turn), `systemMessage` (shown to the user), `hookSpecificOutput` (wraps the per-event fields above under PascalCase-style configs).

Where output is shown: stdout JSON is machine-parsed and can feed the model
(`additionalContext`) or the user (`systemMessage`/deny reasons); nothing in the fetched CLI
docs specifically discusses stderr routing for the CLI surface (VS Code's doc is explicit that
stderr is shown to the model on a blocking exit — see Surface 2 §7).

### 8. Reliability & limits

- Default timeout **30s** (`timeoutSec`/`timeout`), per-hook override.
- Non-zero exit or timeout: **fail-open** except `preToolUse`, which is fail-closed on error
  (not on timeout — timeout is fail-open even for `preToolUse`).
- Malformed hook config: "Keep valid hooks in a config file when one hook entry is malformed"
  (1.0.71) — one bad entry no longer breaks the whole file.
- Missing binary / broken script: not explicitly documented as a distinct case beyond generic
  non-zero-exit fail-open handling.
- Output size: capped at **10 MiB** per invocation (1.0.76), to stop a hook from exhausting
  memory or bloating the session.
- Hooks for one event run **sequentially** (community-confirmed via changelog + third-party
  write-ups, not an explicit "concurrency model" doc section) — not documented as a formal
  guarantee, flagged accordingly.
- Actively buggy in the wild: at least 6 open/closed issues found directly on
  `github/copilot-cli` about hooks not firing or being ignored in specific situations
  (subagents, background agents, plugin-shipped hooks, parallel tool calls) — see §10 sources
  list. This is a young, still-hardening feature, not a finished one.
- Blocking vs fire-and-forget: `preToolUse`/`postToolUse`/`agentStop`/`permissionRequest` are
  blocking (the agent waits on the decision); `notification` is explicitly **asynchronous**
  ("fires asynchronously... " per the reference doc).

### 9. Security posture

- **Folder/workspace trust gates hook execution**: "Repo-level hooks are loaded only after
  folder trust is confirmed, not before the trust dialog is shown" (1.0.8 — this was a
  *fix*, implying an earlier window where hooks could load pre-trust). The general trust
  dialog text (from the CLI overview docs): "Copilot will ask you to confirm that you trust
  the files in this folder... you should only proceed if you trust the files in this
  location," with options to trust for the session only or persistently.
- **Non-interactive / prompt mode is locked down by default**: `-p` mode gates repo hooks and
  workspace MCP behind opt-in env vars `GITHUB_COPILOT_PROMPT_MODE_REPO_HOOKS` and
  `GITHUB_COPILOT_PROMPT_MODE_WORKSPACE_MCP`, explicitly "for secure-by-default behavior"
  (1.0.40); later (1.0.49) repo hooks were allowed to load in `-p` mode once the folder is
  already trusted.
- **Policy-level (enterprise) hooks**: machine-wide, loaded first, cannot be turned off by a
  user's `disableAllHooks`; on POSIX must be root-owned and non-writable by group/other —
  i.e., the one layer designed explicitly to be tamper-resistant against the end user.
  Enterprise-managed hooks + MCP config shipped as part of "Enterprise-managed plugins in
  GitHub Copilot CLI... public preview," 2026-05-06
  (<https://github.blog/changelog/2026-05-06-enterprise-managed-plugins-in-github-copilot-cli-are-now-in-public-preview/>).
- **Explicit vendor guidance treats hooks as a security-sensitive surface**, quoted directly
  from the how-to doc: "Hooks should be treated as security-sensitive code that process agent
  action metadata as input; you should validate that input, avoid unsafe shell construction,
  do not log secrets, and set timeouts, as a hook that is itself injectable is another risk
  surface." (<https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/use-hooks>)
- No separate "first time this repo's hooks run" confirmation beyond the general
  folder-trust dialog (i.e., trusting a folder implicitly trusts its hooks too — there is no
  hooks-specific second gate documented).

### 10. Third-party installability

Realistic and low-friction for the **file-drop** layer: `.github/hooks/*.json` is a directory
glob of independent files, and `~/.copilot/hooks/` likewise — a third-party installer (grim)
can write its own uniquely-named JSON file into either location without touching anything a
human authored, and remove it cleanly later (own-a-whole-file model, since there's no
per-entry ID — see §3). Writing into `settings.json`/`settings.local.json`'s inline `hooks`
key would require true JSON-merge splicing (doable, but a shared-document edit, not a
drop-in file) — the directory-glob path is clearly the friendlier integration seam.

**Restart gotcha, confirmed**: hook config changes "are loaded when the CLI starts" — an
installed/updated hook file will not affect an **already-running** CLI session; the user (or
installer) needs to start a new session for it to take effect. No live-reload for the CLI
(contrast VS Code, §10 below).

### 11. Trampoline viability

**Strong candidate.** A single command hook entry
(`{"type":"command","command":"grim hook run --client copilot --event <E>"}`) fits the schema
directly: stdin carries a well-typed JSON payload per event, stdout is parsed as JSON with a
documented per-event response shape, exit code semantics are well-defined (if asymmetric:
fail-closed only for `preToolUse`). Concrete blockers/asymmetries a portable schema must
absorb, not blockers to a trampoline *existing*:

- Two casing/payload dialects (native camelCase vs "VS Code-compatible" snake_case) are
  selected by which casing you register the event under — a trampoline must pick one
  consistently (PascalCase/snake_case is the more cross-client-portable choice, since VS Code
  and Claude Code both use it).
- No stable per-entry ID — grim must own whole files (already grim's general pattern for
  other clients), not entries inside a shared array.
- The CLI needs a session restart to see a newly-materialized hook file; the underlying `grim`
  binary itself is a normal cross-platform executable, so the *command string* needs no
  `bash`/`powershell` split — `command: "grim hook run ..."` alone covers Windows/macOS/Linux,
  which sidesteps one whole axis of native complexity.
- `preToolUse`'s fail-closed-on-error behavior means a trampoline that crashes or is missing
  from `$PATH` **denies the tool call** rather than silently allowing it — a correctness-critical
  detail to replicate deliberately, not by accident.
- Hooks are **not** covered by the new cross-vendor "Agent Plugins 1.0" open standard (see
  Cross-cutting notes) — there is no existing portable spec to defer to; grim would be filling
  a genuine gap, not re-implementing one that already exists.

---

## Surface 2 — VS Code embedded Copilot agent mode

### 1. Existence & name

Exists, vendor name **"Agent hooks"**, explicitly and currently marked **"(Preview)"**:

> "Agent hooks are currently in Preview. The configuration format and behavior might change in
> future releases."
> — <https://code.visualstudio.com/docs/agent-customization/hooks> (fetched 2026-08-14)

First shipped in **VS Code 1.109.3** (January 2026 update train), per the official release
notes (<https://code.visualstudio.com/updates/v1_109>, fetched 2026-08-14): "Agent hooks
(Preview): Hooks let you execute custom shell commands at key lifecycle points during agent
sessions," explicitly noting "The feature uses the same format as Claude Code and Copilot CLI,
allowing configuration reuse across these tools." A second, narrower preview layer —
**agent-scoped hooks** declared in a custom agent's own `.agent.md` frontmatter, gated behind
the `chat.useCustomAgentHooks` setting — shipped later, in **VS Code 1.111** (2026-03-09,
<https://code.visualstudio.com/updates/v1_111>, fetched 2026-08-14). The base
(non-agent-scoped) mechanism needs **no setting to turn on** — dropping a file under
`.github/hooks/` is picked up automatically.

Organizations can disable the whole feature: "Your organization might have disabled the use
of hooks in VS Code."

### 2. Config location(s)

| Scope | Path |
|---|---|
| Workspace (native) | `.github/hooks/*.json` |
| Workspace (Claude-format, also read) | `.claude/settings.json`, `.claude/settings.local.json` |
| User | `~/.copilot/hooks`, `~/.claude/settings.json` |
| Custom-agent-scoped | `hooks` key in a `.agent.md` file's YAML frontmatter |

Format: JSON. Locations are configurable/extendable via the `chat.hookFilesLocations` setting
(accepts folders or individual files, absolute/`~`-relative), and hook discovery can walk up
into parent repositories via `chat.useCustomizationsInParentRepositories`. **Same directory**
(`.github/hooks/`) as the standalone CLI reads — meaning a repo can plausibly serve both
surfaces from one file, subject to each surface's own event-set support (§4).

**Reload behavior — the opposite of the CLI**: "Save the file and it is automatically loaded
by VS Code" — **no restart or window reload needed**; this is a live, not snapshotted,
mechanism.

### 3. Config schema — verbatim

Same top-level shape as the CLI's native format — **named map keyed by event, values are
arrays**:

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "type": "command",
        "command": "./scripts/block-dangerous.sh",
        "timeoutSec": 5
      }
    ],
    "PostToolUse": [
      {
        "type": "command",
        "command": "./scripts/format-changed-files.sh",
        "windows": "powershell -File scripts\\format-changed-files.ps1",
        "timeout": 30
      }
    ]
  }
}
```

Hook entry fields: `type` (**`"command"` is the only type available right now** — no
`http`/`prompt` types in VS Code, unlike the CLI), `command` (required), `timeout`/`timeoutSec`
(default 30s), `cwd`, `env`, and platform overrides `windows`/`linux`/`osx`. **No `id`/`name`/
`description` field** here either — same "own the whole file" conclusion as the CLI.

### 4. Event catalogue (VS Code's 8, verbatim PascalCase names)

1. `SessionStart` — first prompt of a new agent session
2. `UserPromptSubmit` — user submits a prompt
3. `PreToolUse` — before the agent invokes any tool
4. `PostToolUse` — after a tool completes successfully
5. `PreCompact` — before conversation context is compacted
6. `SubagentStart` — a subagent is spawned
7. `SubagentStop` — a subagent completes
8. `Stop` — the agent session ends

This is a **strict subset** of the CLI's 14 — VS Code has no `notification`,
`permissionRequest`, `sessionEnd`, `userPromptTransformed`, `postToolUseFailure`,
`preMcpToolCall`, or `errorOccurred` (not documented as present; treat as **NOT DOCUMENTED /
absent** for this surface rather than assuming parity with the CLI).

### 5. Invocation

Shell command string (`command`, plus OS-specific `windows`/`linux`/`osx` overrides) run by
VS Code itself — "Hooks execute shell commands with the same permissions as VS Code." Working
directory defaults to the workspace, overridable via `cwd`. Ordering/concurrency: **NOT
DOCUMENTED** on the fetched page (no statement either way about parallel vs. sequential
execution for multiple hooks on one event).

### 6. Input payload — verbatim

JSON on stdin, common envelope:

```json
{
  "timestamp": "ISO 8601 timestamp",
  "cwd": "working directory",
  "session_id": "unique session identifier",
  "hook_event_name": "event name",
  "transcript_path": "path to session transcript"
}
```

Plus event-specific fields (`tool_name`, `tool_input`, `prompt`, etc. — exact per-event key
lists beyond the common envelope were **NOT DOCUMENTED** in the fetched page in full; the page
describes them as "vary per event type" without enumerating every key for every event).

### 7. Output / response contract — verbatim

```json
{
  "continue": true,
  "stopReason": "reason for stopping",
  "systemMessage": "warning message to user",
  "hookSpecificOutput": {
    "permissionDecision": "allow|deny|ask",
    "permissionDecisionReason": "explanation",
    "additionalContext": "context to inject"
  }
}
```

- `continue: false` stops processing; `stopReason` is shown when it does.
- `systemMessage` is shown to the user.
- `hookSpecificOutput.permissionDecision` (`PreToolUse`) controls allow/deny/ask.
- `PostToolUse` can also return `decision: "block"`.

Exit codes:

| Code | Behavior |
|---|---|
| `0` | Success — stdout parsed as JSON |
| `2` | Blocking error — stop processing, **stderr is shown to the model as context** |
| other | Non-blocking warning — shown to the user, execution continues |

/ stderr routing is explicit here (unlike the CLI doc): on a blocking (`2`) exit, stderr goes
to the model.

### 8. Reliability & limits

Default timeout 30s (`timeout`/`timeoutSec`), override per hook. Exit-code fail-open/closed
semantics as above. Parallelism/ordering for multiple hooks on the same event: **NOT
DOCUMENTED**. No stated output-size cap (unlike the CLI's 10 MiB limit) — **NOT DOCUMENTED**,
treat as unconfirmed rather than assuming the same limit applies.

### 9. Security posture

Explicit vendor warning, quoted verbatim:

> "Hooks execute shell commands with the same permissions as VS Code. Review hook
> configurations carefully, especially when using hooks from untrusted sources."

Plus stated recommendations: review hook scripts before enabling, apply least privilege,
validate/sanitize hook input, never hardcode secrets. Workspace Trust is the umbrella gate:
opening an untrusted workspace in restricted mode disables agent mode entirely (and therefore
hooks with it) — VS Code's security doc states "Workspace Trust boundary...disables agents in
that workspace," but this is a general agent-mode gate, not a hooks-specific second prompt.

### 10. Third-party installability

Realistic: `.github/hooks/*.json` is the same directory-glob pattern as the CLI, and — unlike
the CLI — **no restart is needed**: "Save the file and it is automatically loaded by VS Code."
This makes VS Code the more forgiving surface for an external installer to materialize into:
write the file, and it's live. Custom-agent-scoped hooks (inside `.agent.md` frontmatter)
would instead require editing a specific agent file's YAML block, a different (document-splice
rather than drop-in-file) integration seam, and sit behind the `chat.useCustomAgentHooks`
setting besides.

### 11. Trampoline viability

Same shape of answer as the CLI (command-type hook, JSON stdin/stdout, exit-code semantics) —
**viable**, with the caveat that VS Code supports only the **`command`** type (no `http`/
`prompt` escape hatches the CLI has), and only 8 of the CLI's 14 events, so a portable grim
schema must treat VS Code as the **narrower** target and gracefully drop/no-op events it
doesn't support rather than assuming the CLI's superset applies everywhere. The "(Preview)"
status and the explicit "format and behavior might change" warning mean any trampoline built
against today's shape should expect upstream churn.

---

## Surface 3 — Copilot cloud coding agent

### 1. Existence & name

Two genuinely different things exist here, and the brief's premise undersells the first one:

- **`copilot-setup-steps.yml`** — a real GitHub Actions workflow file
  (`.github/workflows/copilot-setup-steps.yml`, job must be named exactly
  `copilot-setup-steps`) that provisions the agent's environment (install deps, warm caches)
  *before* the agent session starts. This is **not** an event-driven hook in the brief's
  sense — it's a one-shot provisioning step, confirmed by
  <https://docs.github.com/en/copilot/how-tos/use-copilot-agents/coding-agent/customize-the-agent-environment>
  (fetched 2026-08-14): allowed keys are only `steps`, `permissions`, `runs-on`, `container`,
  `services`, `snapshot`, `timeout-minutes` (≤ 59); "If a custom setup step fails, Copilot
  will skip the remaining setup steps and begin working with the current state" (fail-open,
  one-shot, no lifecycle events beyond "runs once before the agent starts").
- **The same "Hooks" mechanism as the CLI, ported to the cloud agent** — this is the part the
  brief's framing missed. Confirmed by two dedicated official docs:
  <https://docs.github.com/en/copilot/concepts/agents/cloud-agent/about-hooks> and
  <https://docs.github.com/en/copilot/how-tos/copilot-on-github/customize-copilot/customize-cloud-agent/use-hooks>
  (both fetched 2026-08-14). Vendor name is still just **"Hooks."** No preview/beta banner
  found on either page.

So: **`copilot-setup-steps.yml` is not the only event surface for the cloud agent** — real,
multi-event hooks exist for it too, sharing config format with the CLI.

### 2. Config location(s)

Single location, **no user/global scope** (there is no "your machine" for a cloud job):
`.github/hooks/*.json` in the repository. Explicit and important gotcha, quoted verbatim:

> "The hooks configuration file must be present on your repository's default branch to be used
> by Copilot cloud agent."

(mirrors the identical default-branch requirement already known for `copilot-setup-steps.yml`
— a feature-branch-only hooks file is invisible to the cloud agent until merged).

### 3. Config schema — verbatim

Identical shape to the CLI's — named map keyed by event, arrays of entries, same `type:
"command"` shape with `bash`/`powershell`/`cwd`/`env`/`timeoutSec`. Example from the official
how-to doc:

```json
{
  "version": 1,
  "hooks": {
    "sessionStart": [
      {
        "type": "command",
        "bash": "echo \"Session started: $(date)\" >> logs/session.log",
        "powershell": "Add-Content -Path logs/session.log -Value \"Session started: $(Get-Date)\"",
        "cwd": ".",
        "timeoutSec": 10
      }
    ],
    "userPromptSubmitted": [
      {
        "type": "command",
        "bash": "./scripts/log-prompt.sh",
        "powershell": "./scripts/log-prompt.ps1",
        "cwd": "scripts",
        "env": { "LOG_LEVEL": "INFO" }
      }
    ]
  }
}
```

No `id`/`name`/`description` field here either.

### 4. Event catalogue (cloud agent)

Per the concept doc, listed without explicitly partitioning CLI-vs-cloud support, but cross-
referenced against the unified reference doc's explicit cloud-agent carve-outs:

- `sessionStart`, `sessionEnd`, `userPromptSubmitted`, `preToolUse`, `postToolUse`,
  `agentStop`, `subagentStop`, `errorOccurred` are usable.
- **`notification` does not fire under Copilot cloud agent** (no interactive user to notify).
- **`permissionRequest` does not apply under Copilot cloud agent** — "tool calls there are
  pre-approved" (no interactive approval loop in an unattended cloud job).
- Whether `preCompact`, `subagentStart`, `postToolUseFailure`, `preMcpToolCall`,
  `userPromptTransformed` are supported cloud-side is **NOT DOCUMENTED** explicitly either
  way in the fetched pages — do not assume parity with the CLI's full 14.

### 5. Invocation

`bash` (Unix) or `powershell` (Windows) field, matching the field actually present — cloud
agent execution is **Linux-only**, so in practice only `bash` is honored; `powershell` is
ignored there. Working directory: `/workspace` (the cloned repo) or `/root`; `cwd` in a hook
entry is relative to the repository root. Filesystem is **ephemeral** — destroyed when the job
ends. Network is **restricted by a firewall**; only GitHub/Copilot hosts are reachable by
default (the how-to doc's own caution: "Be cautious with hooks that make external network
calls"). Env vars set in the job: `GITHUB_COPILOT_API_TOKEN`, `GITHUB_COPILOT_GIT_TOKEN`,
`COPILOT_AGENT_PROMPT`, `HOME=/root`; notably **`GITHUB_TOKEN` is not set**. Default timeout
30s (`timeoutSec`), same as the other surfaces.

### 6. Input payload / 7. Output contract

Same JSON-on-stdin / JSON-on-stdout contract as the CLI (§6–7 above) for the events that do
apply; no cloud-specific payload differences were found in the fetched docs beyond the
missing-events carve-outs in §4.

### 8. Reliability & limits

No cloud-specific timeout/limit numbers beyond what's shared with the CLI (30s default,
presumably the same 10 MiB output cap, though that specific number was stated in the CLI
changelog rather than the cloud-agent doc — treat the 10 MiB figure as **CLI-confirmed,
cloud-unconfirmed** pending a cloud-specific source). Because the job is non-interactive,
`ask`-style permission decisions have nowhere to go — the reference doc says the CLI treats
a `preToolUse` `"ask"` decision as `"deny"` under the cloud agent specifically (no one to ask).

### 9. Security posture

No cloud-agent-specific trust dialog (there's no interactive session to show one to) — the
gating instead happens at the repository level: hooks only take effect once merged to the
default branch (§2), and the execution sandbox itself is the isolation boundary (ephemeral
container, restricted network, `/workspace` scoped filesystem). The same general "treat hooks
as security-sensitive code" guidance from the CLI how-to doc (§9 above) is the operative
vendor wording; no additional cloud-specific security callout was found in the fetched pages.

### 10. Third-party installability

Realistic at the **file** level exactly like the CLI/VS Code (`.github/hooks/*.json`,
directory glob, own-a-whole-file model, no per-entry ID) — but the propagation path is
different: since this surface has no long-running local process to "restart," the real
analog to a snapshot/reload gotcha is **the default-branch requirement**: a hook file grim
materializes on a feature branch is inert for the cloud agent until that branch is merged.
An installer targeting this surface needs to account for that lag explicitly (unlike VS Code's
immediate pickup, or even the CLI's "next session" pickup).

### 11. Trampoline viability

Same viability case as the CLI (§11 above) — same schema, same command-hook mechanism — with
extra constraints to design around: Linux/`bash`-only (no `powershell` path matters here),
no interactive events (`notification`, `permissionRequest` are moot), a restricted network
egress (an HTTP-type hook or an `http`-calling trampoline needs to reach an allow-listed host),
and the default-branch propagation delay above. A `grim hook run --client copilot --event <E>`
trampoline binary would need to itself be resolvable inside the ephemeral container (i.e.,
either pre-installed via `copilot-setup-steps.yml`, or self-contained/vendored some other way)
— worth flagging as the one real blocker specific to this surface: **the trampoline binary
itself has no guaranteed presence in the cloud sandbox unless something puts it there first**,
and `copilot-setup-steps.yml` (best-effort, fail-open, one-shot) is the only documented lever
to do that.

---

## Cross-cutting: "Agent Plugins 1.0" and what it does/doesn't standardize

On 2026-08-06, GitHub (with AWS, Anysphere/Cursor, Microsoft, OpenAI, Vercel, and later Google)
published **Agent Plugins 1.0**, an open, cross-vendor packaging standard — spec at
<https://github.com/agentplugins/agent-plugins-spec/blob/main/spec/1.0.0.md>, announced at
<https://github.blog/changelog/2026-08-12-agent-plugins-1-0-in-vs-code-copilot-cli-and-the-copilot-app/>
(fetched 2026-08-14). This matters directly to the "portable schema" motivation behind this
research, so it's worth stating precisely: **the standard packages skills and MCP servers, not
hooks.** The announcement's own words: "Custom agents, commands, rules, and hooks load from
there across VS Code, Copilot CLI, and the Copilot app" — but hooks are named as a
**Copilot-specific capability**, living under a namespaced `com.github.copilot/` directory
inside a plugin, **outside** the standardized portable core. In other words: GitHub itself
already had the exact ambition grimoire has (a portable cross-client hook mechanism) on its
radar via its own Claude-format compatibility shims (Surface 1 §2/§6), but when it came time
to publish a real open standard with other vendors, hooks were explicitly left out of scope.
There is no existing cross-vendor spec to defer to for hooks specifically — confirmed gap, not
an oversight in this research.

## Sources

| URL | What it establishes | Fetched |
|---|---|---|
| https://docs.github.com/en/copilot/reference/hooks-reference | Unified CLI+cloud-agent hook reference: event catalogue, schema, payloads, exit codes, matcher syntax, HTTP/prompt hook types, notification/permissionRequest cloud carve-outs | 2026-08-14 |
| https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/use-hooks | CLI how-to: security guidance quote, worked example, "loaded when CLI starts" reload note | 2026-08-14 |
| https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference | `~/.copilot` directory layout, `COPILOT_HOME`/`COPILOT_CACHE_HOME`, project-level `.github/copilot/settings*.json` | 2026-08-14 |
| https://docs.github.com/en/copilot/concepts/agents/cloud-agent/about-hooks | Cloud-agent hook concept doc: event list, execution environment | 2026-08-14 |
| https://docs.github.com/en/copilot/how-tos/copilot-on-github/customize-copilot/customize-cloud-agent/use-hooks | Cloud-agent how-to: worked examples, default-branch requirement (verbatim) | 2026-08-14 |
| https://docs.github.com/en/copilot/how-tos/use-copilot-agents/coding-agent/customize-the-agent-environment | `copilot-setup-steps.yml`: file path, required job name, allowed keys, fail-open one-shot behavior | 2026-08-14 |
| https://code.visualstudio.com/docs/agent-customization/hooks | VS Code Agent hooks (Preview): schema, 8 events, settings, security warning, exit codes, stderr-to-model | 2026-08-14 |
| https://github.com/microsoft/vscode-copilot-chat/blob/main/assets/prompts/skills/agent-customization/references/hooks.md | VS Code's in-repo source-of-truth hook reference (corroborates the docs site) | 2026-08-14 |
| https://code.visualstudio.com/updates/v1_109 | First VS Code ship date/version for Agent hooks (Preview): "1.109.3", explicit Claude Code/Copilot CLI format-compatibility statement | 2026-08-14 |
| https://code.visualstudio.com/updates/v1_111 | Agent-scoped hooks in `.agent.md` frontmatter, `chat.useCustomAgentHooks` setting, dated 2026-03-09 | 2026-08-14 |
| https://code.visualstudio.com/docs/agents/security | VS Code Workspace Trust boundary disabling agents (and hence hooks) in untrusted workspaces | 2026-08-14 |
| https://github.com/github/copilot-cli/blob/main/changelog.md (fetched raw via GitHub API) | Full version-by-version hook feature history, v0.0.396 (2026-01-27, first `preToolUse` hook) through v1.0.80 (2026-08-14) — ~40 hook-related entries used to build the timeline table | 2026-08-14 |
| https://github.com/github/copilot-cli/issues/2893 | Open reliability bug: `preToolUse` hooks dispatched serially with gaps under parallel tool calls, timeout falls back to allow. State OPEN, opened 2026-04-22, updated 2026-06-19 | 2026-08-14 |
| https://github.com/github/copilot-cli/issues/1730, /2540, /2293, /3659, /2585 | Corroborating open bugs: hooks not firing for sessionStart, plugin-shipped hooks, background agents, and additionalContext not passed through — evidence the feature is young/still-hardening | 2026-08-14 |
| https://github.com/github/copilot-cli/issues/2013 | Closed bug (opened 2026-03-13, closed 2026-04-10): `updatedInput` ignored — shows the doc/implementation gap gets fixed over time | 2026-08-14 |
| https://github.blog/changelog/2026-02-25-github-copilot-cli-is-now-generally-available/ | CLI GA date, for context on how early hooks (Jan 2026) predate GA (Feb 2026) | 2026-08-14 |
| https://github.blog/changelog/2026-05-06-enterprise-managed-plugins-in-github-copilot-cli-are-now-in-public-preview/ | Enterprise-managed/policy-level hooks + MCP config, public preview date | 2026-08-14 |
| https://github.blog/changelog/2026-08-12-agent-plugins-1-0-in-vs-code-copilot-cli-and-the-copilot-app/ | Agent Plugins 1.0 announcement; explicit statement that hooks sit outside the portable standardized core | 2026-08-14 |
| https://github.com/agentplugins/agent-plugins-spec/blob/main/spec/1.0.0.md | The actual open cross-vendor spec (referenced for scope confirmation, not deep-read) | 2026-08-14 |
