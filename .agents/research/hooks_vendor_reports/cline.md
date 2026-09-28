# Cline — native hook / lifecycle-event mechanism

Research date: **2026-08-14**. All URLs fetched on this date unless noted. Primary sources:
official docs (docs.cline.bot), the official blog (cline.bot/blog), and the public
`cline/cline` GitHub monorepo (source code + CHANGELOG.md + git tags/commit dates), read at
the `main` branch HEAD, which at fetch time matches the currently-published extension version
(`apps/vscode/package.json` → `"version": "4.1.10"`, matching the top CHANGELOG.md entry).

**Client scoped**: the VS Code/JetBrains **IDE extension** ("cline", npm/VSIX package name
`claude-dev`), source under `apps/vscode/` in the monorepo. See the important JetBrains caveat
in §10/§11 — all source evidence below is from `apps/vscode/`; no `apps/jetbrains` directory
exists in the public monorepo.

---

## 0. Two distinct "hooks"-named systems — disambiguation

The repo and docs use "hooks" for **two different, non-interoperable mechanisms**. This report
covers only the first; the second is named only to prevent confusion.

1. **File Hooks** (this report's subject) — external, user-supplied executable scripts
   discovered by filename-equals-event-name convention, run as child processes, JSON on
   stdin/stdout. Shipped in the VS Code/JetBrains-facing extension since v3.36.0
   (2025-11-06). Directory: `.clinerules/hooks/` (workspace) and `~/Documents/Cline/Hooks/`
   (global).
2. **Runtime Hooks** — typed, in-process TypeScript/JavaScript lifecycle callbacks
   (`beforeRun`, `afterRun`, `beforeModel`, `afterModel`, `beforeTool`, `afterTool`, `onEvent`)
   declared inside a **Plugin** (`AgentPlugin.hooks`), part of the newer **Cline Plugins**
   system shipped in v4.0.0 (2026-06-26, monorepo tag `v4.0.0` → commit
   `1e88a708bde9`, committer date `2026-06-26T19:50:52Z`). Plugins install via
   `cline plugin install` (npm/git/file-URL/local) into `.cline/plugins/` (project) or
   `~/.cline/plugins/` (global). **Per the official docs**
   (`https://docs.cline.bot/sdk/plugin-install.md`, fetched 2026-08-14): *"This feature
   currently only applies to Cline SDK, CLI, and Kanban. This feature is not applicable on
   VSCode and JetBrains Extension for now."* — i.e., Plugins/Runtime Hooks are **out of scope**
   for the assigned client. The repo's own `sdk/examples/hooks/README.md` further muddies
   naming by calling File Hooks "**an adapter on top of the runtime hook layer**" for the
   **CLI/SDK** surface specifically (`.cline/hooks/`, different directory and different output
   schema than the VS Code extension's `.clinerules/hooks/` — see the discrepancy note in §3).

The current top-level docs page for Hooks (`https://docs.cline.bot/features/hooks`, and its
markdown twin `https://docs.cline.bot/customization/hooks.md`) is now just a stub: *"See
details under SDK Plugins page"*, linking to `/sdk/plugins`. This is misleading/stale
documentation-information-architecture — the VS Code/JetBrains-facing File Hooks feature is
fully alive in the shipped extension (confirmed directly from `main`-branch source, matching
the published `4.1.10` version) but the public docs site no longer gives it a dedicated page;
the closest living canonical doc is the file shipped **inside the repo itself**,
`.clinerules/hooks/README.md` (the maintainers' own dogfooded hooks doc — see §9 for staleness
caveats even in that file).

---

## 1. Existence & name

**VERDICT-relevant fact: Cline has a real, shipped, native hook mechanism for the VS Code/
JetBrains-facing extension**, called **"Hooks"** (capital H; file/event names are
PascalCase, e.g. `PreToolUse`).

- Shipped: **v3.36.0**, tag `v3.36.0`, commit `9de4fc2a2f52…`, committer date
  **2025-11-06T02:10:09Z** (`gh api repos/cline/cline/commits/9de4fc2a2f52`, fetched
  2026-08-14). Confirmed independently by the official blog post
  `https://cline.bot/blog/cline-v3-36-hooks` ("Cline v3.36: Hooks — Inject Custom Logic Into
  Cline's Workflow"), also mirrored at `https://cline.ghost.io/cline-v3-36-hooks/`.
- Stability: **not labeled beta/experimental** in any doc or changelog entry found. Presented
  as a regular shipped feature. It **is** gated by a user-facing on/off setting (see §9) whose
  default is **enabled** as of the current `main`/4.1.10 source
  (`getHooksEnabledSafe(userSetting) => userSetting ?? true`,
  `apps/vscode/src/core/hooks/hooks-utils.ts`).
- Deprecation/rework history (from `CHANGELOG.md`, fetched 2026-08-14, dates resolved via git
  tag → commit → committer date):
  - **v3.36.0** (2025-11-06): "Add: Hooks allow you to inject custom logic into Cline's
    workflow." (initial ship)
  - **v3.68.0** (2026-02-26): "Hooks: Hook scripts now run from the workspace repository root
    instead of filesystem root." (behavior-breaking fix to cwd handling)
  - **v3.70.0** (2026-03-04): "Hook payloads now include `model.provider` and `model.slug`."
    (input-schema addition)
  - **v3.71.0** (2026-03-06): "Hooks: Added a `Notification` hook for attention and completion
    boundaries." (new event type)
  - **v3.72.0** (2026-03-12): "Hooks: reintroduced feature toggle." — implies the on/off
    setting was **removed at some point between 3.36 and 3.72 and then reintroduced**; the
    current source shows the reintroduced toggle defaults to **on**.
  - **v4.0.0** (2026-06-26): ships the unrelated **Cline Plugins** system (see §0) alongside
    Hooks; Hooks are not deprecated by this.
  - **v4.1.7** and **v4.1.10** changelog entries mention Plugins running in a "sandbox"
    process with "atomic plugin toggles" and "idle plugin sandbox process" reclamation — this
    sandboxing applies to **Plugins**, not to File Hooks, which (per source read in §5/§9) run
    as ordinary unsandboxed child processes.
  - A currently-live migration: the **v4.1.10** CHANGELOG.md header states *"Everything in
    this release lands through the SDK bundle, so it applies to windows running that bundle
    and not the legacy one. The legacy bundle is unchanged from 4.1.9."* Cline is mid-rollout
    from a legacy extension architecture to a new SDK-runtime-backed bundle. This matters for
    Hooks: see §4 and §11 for which File Hook events are wired in the new bundle's adapter as
    of this snapshot.
  - `shouldContinue` (boolean) is a **removed/deprecated output field**; source
    (`hook-factory.ts`) contains a hard validation error if a hook script still returns it,
    with an explicit migration message: *"The 'shouldContinue' field has been removed. Use
    'cancel: true' instead... Before: { shouldContinue: false, errorMessage: '...' } / After:
    { cancel: true, errorMessage: '...' }"*. This confirms an earlier, now-invalid schema
    generation existed pre-dating the currently documented `cancel` field.

---

## 2. Config location(s)

Two scopes, **both file-based directories**, no JSON/TOML/YAML config file at all — the
"config" *is* the presence of an appropriately-named, appropriately-permissioned file.

| Scope | Path (Unix) | Path resolution in source |
|---|---|---|
| Global (all workspaces) | `~/Documents/Cline/Hooks/` | `apps/vscode/src/core/hooks/utils.ts`: `path.join(os.homedir(), "Documents", "Cline", "Hooks")` |
| Project/workspace | `<workspace-root>/.clinerules/hooks/` | same file: `path.join(cwd, ".clinerules", "hooks")`; for multi-root workspaces, resolved per named root via `HostProvider.workspace.getWorkspacePaths` |

No environment variable relocates either directory in the VS Code/JetBrains extension (no
`GLOBAL_HOOKS_DIR`-style override was found in the reviewed source — `resolveHooksDirectory()`
does accept a `globalHooksDirOverride` parameter but it is a function argument used only by
tests, not an env var read at runtime).

Directory *is* the discovery convention — no glob of arbitrary filenames; only exact,
event-name-matching filenames are ever considered (§3). No separate manifest/index file lists
which hooks are active; presence + executable bit (Unix) or presence as `<Name>.ps1` (Windows)
**is** the registration.

**Merging**: global and workspace hooks are **not** "one wins" — the source's
`CombinedHookRunner` (see §7) runs **both concurrently and merges results**. In a multi-root
workspace, each root's own `.clinerules/hooks/` is also merged in (concurrently, no ordering
guarantee) — quoting `.clinerules/hooks/README.md`: *"If you have multiple workspace roots,
you can place hooks in each root's `.clinerules/hooks/` directory. All hooks (global and
workspace) may execute concurrently... No execution order is guaranteed between hooks from
different directories."*

Only **one physical file per event name per directory** is ever discovered — there is no
`PreToolUse.d/`-style multi-file convention within a single directory. (`findUnixHook`/
`findWindowsHook` in `hook-factory.ts` check exactly one candidate path:
`path.join(hooksDir, hookName)` on Unix, `path.join(hooksDir, hookName + ".ps1")` on Windows.)

A separate, **incompatible** CLI/SDK-surface convention also exists (out of scope per §0, but
worth flagging for anyone building a portable schema): `.cline/hooks/` (project) and
(implied) `~/.cline/hooks/` (global), loadable also from an arbitrary directory via
`cline --hooks-dir <path>`. Source: `sdk/examples/hooks/README.md`,
`sdk/examples/hooks/PreToolUse.py` docstring ("Copy to `~/.cline/hooks/PreToolUse.py`").

---

## 3. Config schema — verbatim

**Neither a named map nor an array.** The "collection of hooks" is **the set of files present
on disk** whose filename equals a recognized event name. There is no single config document to
quote a JSON/YAML shape for. The closest thing to a schema is the **allow-list of valid
filenames**, taken verbatim from `apps/vscode/src/core/hooks/utils.ts`:

```ts
export const VALID_HOOK_TYPES = [
	"TaskStart",
	"TaskResume",
	"TaskCancel",
	"TaskComplete",
	"PreToolUse",
	"PostToolUse",
	"UserPromptSubmit",
	"Notification",
	"PreCompact",
] as const
```

Platform-specific filename rule (verbatim comment + code, same file):

> "On Windows, only PowerShell-native naming is supported: `<HookName>.ps1`... On Unix-like
> platforms (Linux/macOS), only canonical extensionless names are considered: `<HookName>`...
> `.ps1` files are not part of the supported Unix hook contract."

```ts
export async function resolveExistingHookPath(hooksDir: string, hookName: string): Promise<string | undefined> {
	const candidates = process.platform === "win32" ? [path.join(hooksDir, `${hookName}.ps1`)] : [path.join(hooksDir, hookName)]
	...
}
```

**Stable identity for a third-party installer**: there is no id/name/description metadata
field anywhere — the **file path itself** (scope directory + exact event-name filename) *is*
the entry's identity. A third-party installer (e.g. grim) can own/update/remove one entry
idempotently simply by owning that exact file path — but see §11: only **one** file per
(scope, event) pair can exist, so an installer must either be the sole owner of that path or
implement its own internal multi-hook fan-out/dispatch inside the one file it owns.

**Matcher/filter syntax**: none. A hook file is unconditionally invoked for *every*
occurrence of its event (every tool call for `PreToolUse`/`PostToolUse`, regardless of tool
name; every prompt submit for `UserPromptSubmit`, etc.). Filtering by tool name or file path
glob is the hook script's own responsibility, done by inspecting the JSON payload it receives
(see the `.clinerules/hooks/README.md` examples that grep `tool_name`/`path` after parsing
stdin with `jq`).

**Discrepancy to flag** (both are "primary source," they disagree — a real, current
divergence, not a stale-doc artifact): the CLI/SDK-surface `sdk/examples/hooks/README.md`
documents an **output** schema of `{cancel, review, context, errorMessage, overrideInput}` —
different field names (`context` not `contextModification`, plus `review` and
`overrideInput`, which the VS Code extension does not implement at all). The VS Code
extension's actual, current, code-validated schema (§7) is only
`{cancel, contextModification, errorMessage}`. **Do not conflate the two** when modeling a
portable schema — a `grim` "cline" hook target must pick one integration surface (this report
is scoped to the IDE extension) and use its exact fields.

---

## 4. Event catalogue

All nine File Hook events, verbatim from `apps/vscode/src/core/hooks/templates.ts` (the
generator that produces each template's doc header) and cross-checked against
`.clinerules/hooks/README.md`:

| Event (exact filename) | Fires when | Group |
|---|---|---|
| `TaskStart` | A **new** task begins (not on resume) | session lifecycle |
| `TaskResume` | An **existing** task is resumed after interruption | session lifecycle |
| `TaskCancel` | A task is cancelled by the user (or hook aborted) mid-work; **not itself cancellable** | session lifecycle / stop |
| `TaskComplete` | A task completes | session lifecycle / stop |
| `UserPromptSubmit` | The user submits a prompt/message (initial task, resume, or feedback) | prompt submit |
| `PreToolUse` | Before any tool executes (`read_file`, `write_to_file`, `execute_command`, …) | pre-tool-use |
| `PostToolUse` | After any tool completes (success or failure) | post-tool-use |
| `Notification` | Cline reaches a user-attention or completion boundary (added v3.71.0, 2026-03-06) | notification |
| `PreCompact` | Before conversation context is compacted/truncated | compaction |

No dedicated `SessionStart`/`SessionEnd` or `SubagentStop`/`Error` event names exist in this
catalogue (Cline has no subagent concept at the file-hook layer). `SessionShutdown` and
`TaskError` names appear **only** in the separate CLI/SDK surface (`sdk/examples/hooks/`) and
are explicitly **not** part of `VALID_HOOK_TYPES` for the VS Code/JetBrains extension.

**Time-sensitive wiring caveat** (current as of the v4.1.10 "SDK bundle" migration, see §0):
`apps/vscode/src/sdk/hooks-adapter.ts` bridges File Hooks into the new SDK runtime's typed
lifecycle callbacks and states explicitly, verbatim:

```
// Runtime hooks use typed in-process lifecycle callbacks:
//   TaskStart        -> beforeRun
//   UserPromptSubmit -> beforeRun with the latest submitted user message
//   PreToolUse       -> beforeTool
//   PostToolUse      -> afterTool
//   TaskComplete     -> afterRun when completed
//   TaskCancel       -> afterRun when aborted
//
// Deferred hooks (NOT wired here): TaskResume, TaskError, SessionShutdown,
// PreCompact, Notification.
```

So on hosts already migrated to the new SDK bundle, **`TaskResume`, `PreCompact`, and
`Notification`** hook files would be correctly discovered (they're still in
`VALID_HOOK_TYPES` and have templates) but **are not yet invoked** by this adapter — a live
gap, not a permanent one. The "legacy bundle" (still shipped in parallel per the 4.1.10
changelog note) is expected to fire all nine.

`Notification` is explicitly **observation-only** even where wired — verbatim from its
template docstring (`templates.ts`):

> "Notification hooks are observation-only: `cancel` is ignored by the caller,
> `contextModification` is ignored by the caller, hook failures are non-fatal."

---

## 5. Invocation

- **Not** argv-parameterized, **not** a JS/TS module import, **not** an HTTP endpoint. Each
  hook is spawned as an OS **child process**.
- **Unix (Linux/macOS)**: `child_process.spawn(escapedScriptPath, [], { shell: true, detached:
  true, cwd, windowsHide: true, stdio: ["pipe","pipe","pipe"] })`. `shell: true` means Node
  runs the path through the platform default shell (`/bin/sh`), which is what makes the
  script's own shebang line (`#!/usr/bin/env bash`, `#!/usr/bin/env python3`, `#!/usr/bin/env
  bun`, …) the effective interpreter selector — *any* language works as long as the file has a
  shebang and the executable bit set. `detached: true` puts the child in its own process
  group so the whole tree can be killed via `process.kill(-pid, "SIGTERM"/"SIGKILL")`.
  Path is shell-escaped via a dedicated `escapeShellPath()` (single-quote wrapping with
  `'\''`-style embedded-quote escaping) specifically to survive spaces/quotes in paths like
  `~/Documents/Cline/Hooks/` or a workspace folder named `My Project`.
- **Windows**: spawned directly (no shell) as
  `<powershell-executable> -NoProfile -NonInteractive -ExecutionPolicy Bypass -File
  <scriptPath>`, `shell: false`, `detached: false`. The PowerShell executable path is resolved
  once and cached for 5 minutes (`WINDOWS_HOOK_LAUNCHER_CACHE_TTL_MS = 5 * 60 * 1000`) so
  concurrent hook launches share the lookup. **Note**: this contradicts
  `.clinerules/hooks/README.md`'s own prose ("Windows: Not currently supported") — that prose
  is **stale**; the code on `main` (matching the shipped 4.1.10 version) clearly implements a
  full Windows PowerShell path. Trust the code over that doc file.
- **Working directory**: workspace-scoped hooks run with `cwd` = that specific workspace
  root; global hooks (`~/Documents/Cline/Hooks/`) run with `cwd` = the **primary** workspace
  root (not the hook's own directory, not the filesystem root). This is the direct result of
  the v3.68.0 fix ("Hook scripts now run from the workspace repository root instead of
  filesystem root").
- **`$PATH` handling**: not customized; the child process inherits the extension host
  (VS Code/JetBrains) process's environment, including `$PATH`, via Node's default
  `child_process.spawn` behavior (no explicit `env` option is passed in `HookProcess.ts`, so
  Node's default — full env inheritance — applies). No hook-specific environment variables are
  injected (all event data arrives via stdin JSON only — see §6).
- **Timeout**: fixed at **30000 ms** (`HOOK_EXECUTION_TIMEOUT_MS` / `timeoutMs: number = 30000`
  in both `hook-factory.ts` and `HookProcess.ts`). **Not** documented as user-configurable in
  the shipped extension despite `.clinerules/hooks/README.md` claiming "configurable via
  `HOOK_EXECUTION_TIMEOUT_MS`" — no environment-variable or settings read of that name was
  found anywhere in the reviewed source; it is a plain `const`. Treat the README's
  "configurable" claim as **unverified/likely stale**.
- **Concurrency**: hooks for the **same event** across multiple discovered scripts (global +
  each workspace root) run **in parallel** via `Promise.all` (`CombinedHookRunner`). **No
  ordering guarantee** between them (explicit in the README and implicit in the `Promise.all`
  implementation).
- **Blocking vs fire-and-forget**: **blocking** for `PreToolUse` (the calling code `await`s
  the hook's result before deciding whether to proceed with the tool call) and generally for
  every event **except** `Notification`, which is explicitly fire-and-forget/observation-only
  (§4).
- **Output size cap**: 1 MB combined stdout+stderr per hook process
  (`MAX_HOOK_OUTPUT_SIZE = 1024 * 1024`); further output is silently dropped once exceeded,
  with a one-time `"\n\n[Output truncated: exceeded 1MB limit]"` marker line emitted to the
  live output stream.
- **Cleanup**: a process-tree kill with 2-second grace (`SIGTERM` then `SIGKILL`) is used both
  for user-initiated cancellation and for extension deactivation (`HookProcessRegistry.
  terminateAll()`), preventing zombie processes.

---

## 6. Input payload — verbatim

**JSON on stdin only.** No env vars, no argv, no template interpolation into a command string.
Every event receives a common envelope plus exactly one event-specific nested object whose key
is the camelCase form of the event name. Verbatim common envelope + per-event shapes, from
`.clinerules/hooks/README.md` (cross-checked field-by-field against the live
`templates.ts`/`hook-factory.ts` source, which additionally confirms a `model: {provider,
slug}` field added in v3.70.0 that the README omits):

```json
{
  "clineVersion": "string",
  "hookName": "TaskStart" | "TaskResume" | "TaskCancel" | "TaskComplete" | "UserPromptSubmit" | "PreToolUse" | "PostToolUse" | "PreCompact",
  "timestamp": "string",
  "taskId": "string",
  "workspaceRoots": ["string"],
  "userId": "string",
  "model": { "provider": "string", "slug": "string" },
  "taskStart": { "taskMetadata": { "taskId": "string", "ulid": "string", "initialTask": "string" } },
  "taskResume": {
    "taskMetadata": { "taskId": "string", "ulid": "string" },
    "previousState": { "lastMessageTs": "string", "messageCount": "string", "conversationHistoryDeleted": "string" }
  },
  "taskCancel": { "taskMetadata": { "taskId": "string", "ulid": "string", "completionStatus": "string" } },
  "taskComplete": { "taskMetadata": { "taskId": "string", "ulid": "string" } },
  "userPromptSubmit": { "prompt": "string", "attachments": ["string"] },
  "preToolUse": { "toolName": "string", "parameters": {} },
  "postToolUse": { "toolName": "string", "parameters": {}, "result": "string", "success": true, "executionTimeMs": 0 },
  "preCompact": { "contextSize": 0, "messagesToCompact": 0, "compactionStrategy": "string" }
}
```

(`hookName` enum in the doc omits `Notification`, which was added after that doc section was
last edited — see the staleness note in §9. The real, current `templates.ts` also documents a
much richer `PreCompact` shape actually used —
`{ taskId, ulid, contextSize, compactionStrategy, previousApiReqIndex, tokensIn, tokensOut,
tokensInCache, tokensOutCache, deletedRangeStart, deletedRangeEnd, contextJsonPath,
contextRawPath }` — and the `Notification` shape:
`{ event, source, message, waitingForUserInput, eventVersion, eventId, messageTruncated,
sourceType, sourceId, requiresUserAction, severity }`.)

A concrete `PreToolUse` example, straight from the SDK-side example script's docstring
(`sdk/examples/hooks/PreToolUse.py`, structurally identical in shape to the VS Code extension's
own `preToolUse` nesting):

```json
{
  "hookName": "tool_call",
  "clineVersion": "1.0.0",
  "timestamp": "2026-01-15T10:30:00Z",
  "taskId": "conv-123",
  "workspaceRoots": ["/path/to/repo"],
  "userId": "user",
  "iteration": 1,
  "tool_call": { "id": "call-456", "name": "read_files", "input": { "filePath": "/path/to/file.ts" } }
}
```

(Note: that exact shape — `hookName: "tool_call"`, top-level `tool_call` key — is the **CLI/SDK
surface's** shape, not the VS Code extension's `hookName: "PreToolUse"` / `preToolUse` key.
Included only to make the divergence between the two surfaces impossible to miss.)

Caller-side note on empty values: proto3 JSON serialization drops empty-string defaults by
default; the hook-factory source explicitly patches around this for `UserPromptSubmit` so an
empty prompt still arrives as `"prompt": ""` rather than being omitted — evidence that the
input is built from a Protobuf-typed `HookInput` message internally
(`HookInput.toJSON(input)`, `HookInput.create(...)` in `hook-factory.ts`), even though the
wire format the hook script sees is plain JSON.

---

## 7. Output / response contract — verbatim

**stdout must contain exactly one JSON object.** Verified, current schema (validated field-by-
field in `hook-factory.ts`'s `validateHookOutput()`, matching the Windows PowerShell template's
literal `ConvertTo-Json` output and every Bash/Python template's `echo`):

```json
{
  "cancel": false,
  "contextModification": "",
  "errorMessage": ""
}
```

- `cancel` (boolean, optional, default `false`): *"Required: false to continue, true to
  block execution"* (doc wording) / actually optional at the JSON level — omitted is treated
  as `false`. **Only this field can stop anything**, and only for events that are wired to
  check it (`PreToolUse` blocks the tool call; other events' `cancel` requests task
  cancellation per the doc, except `Notification` where it's ignored, §4).
- `contextModification` (string, optional): injected into the **next** LLM turn, not the
  current one — explicit and repeated emphasis in the docs: *"context injected by hooks
  affects FUTURE AI decisions, not the current tool execution... The hook cannot modify
  [the current tool's] parameters."* Truncated at **50,000 bytes**
  (`MAX_CONTEXT_MODIFICATION_SIZE = 50000`) with an appended
  `"\n\n[... context truncated due to size limit ...]"` marker.
- `errorMessage` (string, optional): shown to the user when `cancel: true`.
- **Removed/invalid field**: `shouldContinue` — see the migration-error text quoted in §1.
  Sending it now **hard-fails validation** regardless of exit code.
- No `systemMessage`, no `ask`/`allow`/`deny` enum (only a boolean), no structural way to
  modify the tool's input before it runs (confirmed absent — `PreToolUse` cannot rewrite
  `parameters`, unlike the separate CLI/SDK surface's undocumented-for-this-client
  `overrideInput` field, §3).

**Exit-code semantics** — this is the single most important nuance, confirmed at **two
independent levels of the source** (the low-level process wrapper and the actual production
call site), not just from a doc claim:

- Exit code alone is **not authoritative**. If stdout parses as valid JSON (matching the
  schema), that JSON is honored **"regardless of exit code"** (verbatim code comment in
  `hook-factory.ts`), even logging a warning if a non-zero exit accompanied valid JSON, but
  still using the JSON.
- If exit code is **0** but stdout has **no parseable JSON**: treated as an implicit
  `{cancel: false}` (allow), with a logged warning — "Completed successfully but no JSON
  response found."
- If exit code is **non-zero** and stdout has **no parseable JSON**: a `HookExecutionError`
  (type `"execution"`) is thrown internally... **but this is fail-open at the real call site**.
  Verbatim, from `apps/vscode/src/sdk/hooks-adapter.ts` (the actual glue between hook execution
  and the tool-approval path):
  ```ts
  } catch (error) {
      emitHookMessage?.(buildHookStatusMessage({ hookName: "PreToolUse", toolName: ctx.toolCall.toolName, status: "failed", ts: runningTs }))
      Logger.error("[HooksAdapter] beforeTool hook failed:", error)
      return undefined   // <-- undefined = "no stop control" = tool proceeds
  }
  ```
  I.e. a crashing, missing-binary, malformed-output, or timed-out hook **does not block the
  tool** — it only surfaces a "failed" status chip in the UI and an error-level log line. Only
  a hook that runs to completion and explicitly returns `{"cancel": true, ...}` blocks
  anything. This matches the class-level doc comment on `StdioHookRunner`
  in `hook-factory.ts`, verbatim: *"Error handling: Treats hooks as 'fail-open': only
  shouldContinue:false [now cancel:true] blocks tool execution. Hook script errors (non-zero
  exit) don't block tools, only explicit JSON response does."*
- **Malformed JSON that still contains a recognizable brace-balanced object** is recovered
  from mixed stdout (debug prints interleaved with the JSON) by a bespoke "scan from the end,
  count braces" extractor in `hook-factory.ts` — so hooks that `echo` debug text to **stdout**
  by mistake (instead of stderr) may still work if a clean JSON object is the trailing content,
  but this is undocumented/best-effort, not a stable contract; hook authors are told
  explicitly (multiple example docs) to use **stderr for logging, stdout reserved for JSON**.
- **Missing binary / not executable**: not discovered at all — `findUnixHook` requires
  `fs.access(candidate, fs.constants.X_OK)` to even consider the file a hook; a non-executable
  file at the right path is silently treated as "no hook here" (an `EACCES`/`ENOENT` from the
  stat/access call is explicitly classified as an **expected, silently-ignored** error in
  `isExpectedHookError()`).

**Where output is shown**: stderr from the hook is surfaced in the VS Code "Cline" Output
channel / hook status UI (via the `HookStreamCallback` "line" events) — visible to the
**user**, not injected into the model's context. stdout is consumed structurally (parsed as
JSON) and only its **`contextModification`** value (if any) is later shown to the **model** (as
future-turn context) — raw stdout text itself is not shown to the user as a chat message,
though the streaming callback does emit stdout lines too (labeled `"stdout"|"stderr"`) for a
live-progress UI treatment ("hook_status" `ClineMessage`s in `hooks-adapter.ts`), so a user
watching the UI does see raw output streaming, not just the model.

---

## 8. Reliability & limits

- **Timeout**: 30,000 ms hard-coded (`HOOK_EXECUTION_TIMEOUT_MS`), not read from any user
  setting or env var in the code reviewed, despite one doc's claim of an env-var override
  (§5). On timeout: `SIGTERM` sent to the process; if it doesn't exit, the process is left to
  the standard `terminate()` 2-second-grace → `SIGKILL` path when cleanup runs.
- **Non-zero exit**: does not block the calling tool/task by itself (fail-open, §7); only
  surfaces as a "failed" hook status in the UI and an `error`-level log line
  (`[HooksAdapter] beforeTool hook failed: ...`).
- **Malformed output**: same fail-open path — logged, tool proceeds, no `cancel` effect.
- **Missing binary / non-executable file**: never discovered as a hook at all — no error
  surfaced to the user; behaves exactly as if no hook were configured (see `EACCES`/`ENOENT`
  handling in §7).
- **Parallelism**: all discovered scripts for one event run **concurrently**
  (`Promise.all`), no ordering guarantee, results merged (§3, §7): `cancel` = logical OR,
  `contextModification` values joined with `"\n\n"`, `errorMessage` values joined with `"\n"`.
- **Blocking**: yes for events wired to gate something (`PreToolUse`); `Notification` is
  explicitly fire-and-forget/non-blocking/non-fatal by design (§4).
- **Output cap**: 1 MB combined stdout+stderr per process (§5); truncated with a one-time
  marker, not an error.
- **Context-injection cap**: 50 KB per merged `contextModification` string (§7).
- **Discovery caching**: results are cached per event name in a process-lifetime singleton
  (`HookDiscoveryCache`), **not** re-scanned on every tool call — but the cache is invalidated
  reactively via **file-system watchers** on every hooks directory (create/change/delete) and
  on workspace-folder-change events, not just at session start. This means adding, editing, or
  deleting a hook file generally takes effect without an extension/window reload. One gap
  worth flagging for a third-party installer: the cache is keyed by **content-change events**
  from the watcher API; whether a **permission-only** change (a bare `chmod +x` with no content
  write) reliably fires the underlying VS Code/Node file watcher is not verified in the source
  reviewed — an installer that writes the file and then `chmod +x`s it as a separate step
  should be safe (the write itself invalidates the cache), but an installer that flips
  permissions on an already-cached "not found" path with no accompanying content write should
  not be assumed to be picked up without verification.
- **Process cleanup**: a global `HookProcessRegistry` tracks every in-flight hook child
  process and force-terminates all of them (graceful `SIGTERM`, 2 s, then `SIGKILL`) on
  extension deactivation, preventing zombies.

---

## 9. Security posture

`.clinerules/hooks/README.md`'s own "Security Considerations" section, quoted verbatim:

> "- Hooks run with the same permissions as VSCode
> - Be cautious with hooks from untrusted sources
> - Review hook scripts before enabling them
> - Consider using `.gitignore` to avoid committing sensitive hook logic
> - Hooks can access all workspace files and environment variables"

This is the vendor's own explicit acknowledgment that hooks are **arbitrary code execution**
with the full privilege of the editor process (no sandbox, no restricted permission set) —
consistent with what the source shows: hooks are launched with plain `child_process.spawn`,
inheriting the full environment, with no seccomp/container/sandbox wrapper. This is in **direct
contrast** to the newer Plugins system, whose v4.1.x changelog entries repeatedly mention
"sandbox processes" and "atomic plugin toggles" — Cline is evidently sandboxing the *newer*
mechanism but has not retrofitted a sandbox onto File Hooks.

**No per-hook or per-repository trust/approval prompt was found** in any of the reviewed
source (`hook-factory.ts`, `HookProcess.ts`, `HookDiscoveryCache.ts`, `hooks-utils.ts`,
`hooks-adapter.ts`, `utils.ts`). Enabling/disabling is a **single global on/off setting**
(`hooksEnabled`, read via `getHooksEnabledSafe()`), not a per-hook allowlist, and — critically —
**its default is `true`** (`userSetting ?? true`) when the user has never touched the setting.
Practically: cloning a hostile repository that ships an already-`+x` `.clinerules/hooks/
PreToolUse` script (git preserves the executable bit) could have that script run automatically
on the very next tool call, for any user who hasn't explicitly turned Hooks off — there is no
workspace-trust-style interstitial found in this code path. (I did not find any reference to
VS Code's own Workspace Trust API — `isTrusted` or similar — anywhere in the hook source
reviewed; I record this as **NOT DOCUMENTED / NOT FOUND**, not as a confirmed absence, since a
gate could plausibly exist elsewhere in the extension activation path that I did not fully
trace.)

No snapshotting-at-session-start behavior was found either — see §8: discovery is cached but
watcher-invalidated continuously, not frozen for the task's lifetime.

Telemetry: every hook discovery and execution emits metadata (not hook content) to Cline's own
telemetry service — `captureHookDiscovery(hookName, globalCount, workspaceCount)` and
`captureHookExecution(taskId, hookName, status, { source, toolName, durationMs, exitCode,
cancelRequested, contextModified, contextSize, errorType, errorMessage })`. Error *messages*
(potentially containing stderr snippets or exception text) do appear to be included in
telemetry payloads on failure paths — a data-handling detail worth noting, though raw stdin/
stdout hook payload content itself was not observed being sent.

**Staleness callout, itself a security-adjacent finding**: the repo's own dogfooded
`.clinerules/hooks/README.md` is measurably behind the shipped code in at least three ways —
(1) claims Windows is unsupported when the code fully implements a PowerShell path; (2) omits
the `Notification` event entirely (added 2026-03-06, after this doc section was last touched);
(3) marks `TaskComplete` and `PreCompact` as "(coming soon!)" when both are fully implemented,
discoverable, and templated in the current source. **Treat the shipped source as ground
truth over any single doc file for this client — including this report's own summaries above,
which should be re-verified against `main` before being hard-coded into a schema.**

---

## 10. Third-party installability

**Yes — realistically installable by editing files alone**, no vendor CLI, no cloud account,
no UI-only step required for the *hook file itself*. Concretely, to install a `PreToolUse`
hook for a given workspace, a third-party tool needs to:

1. Write an executable file at `<workspace>/.clinerules/hooks/PreToolUse` (Unix) and/or
   `<workspace>/.clinerules/hooks/PreToolUse.ps1` (Windows) — plain filesystem writes, no
   special API.
2. `chmod +x` it on Unix (Windows needs no bit — `.ps1` discovery doesn't check
   executability, only file existence, per `isHookFile()` in `hook-factory.ts`, which is a
   plain `fs.stat(...).isFile()` check with no `X_OK` access check on the Windows path).
3. Ensure the **global "Enable Hooks" setting is on** — this is the one piece that is
   **not** purely file-editable in a documented way: it is a VS Code/JetBrains
   **settings-UI checkbox** ("Cline settings → Feature Settings → Enable Hooks"). Because it
   defaults to `true` (§9), a fresh install needs no user action for hooks to run at all — but
   an installer cannot be *certain* the toggle wasn't previously flipped off by the user, and
   there is no documented settings-JSON key confirmed safe to write directly from outside the
   extension (the setting is read via `stateManager.getGlobalSettingsKey("hooksEnabled")`,
   which is Cline's own internal global-state store, not a plain VS Code
   `settings.json` key as far as the reviewed source shows).
4. No restart is required for a **new or edited** hook file to take effect in the common case
   — the `HookDiscoveryCache` is watcher-invalidated (§8), not just snapshotted once at
   startup/session start. A permission-only change with no accompanying watched
   create/modify/delete event is the one scenario not confirmed safe (§8) — recommending a
   window reload as a defensive fallback for an installer is reasonable, not because the docs
   say so, but because the watcher's exact event coverage wasn't verifiable from the source
   read.

**Caveat on scope of verification**: everything above was verified against `apps/vscode/` —
the VS Code extension source — because that is the only implementation present in the public
`cline/cline` monorepo (`apps/` contains `cli`, `cline-hub`, `examples`, `vscode`, and
`vscode-rollout`; **no `apps/jetbrains` directory exists**). Marketing pages
(`https://cline.bot/ide`) advertise "VS Code, JetBrains, Cursor, and Windsurf" support for the
IDE product, and a shared background "Hub daemon" is referenced in the 4.1.10 changelog
("Cline installations on different builds... Hub daemon"), which makes it plausible that
JetBrains is a thin front-end sharing this same hook engine through that daemon — but I found
**no direct source evidence** either way. **Treat JetBrains-specific installability as
unverified (NOT DOCUMENTED in the sources available to this research), not as confirmed
parity with VS Code.**

---

## 11. Trampoline viability

**Favorable, with real but manageable blockers.** A single generic command
(`grim hook run --client cline --event <Event>`) is structurally a very good fit because:

- The hook **is** an executable file — no JS/TS module loading, no in-process API, no HTTP
  round-trip. Any shebang-capable executable works on Unix; any `.ps1` works on Windows.
  grim's trampoline body is trivially `#!/usr/bin/env sh\nexec grim hook run --client cline
  --event PreToolUse` (Unix) or a two-line PowerShell wrapper calling `grim.exe hook run ...`
  (Windows).
- All event data arrives over **stdin as JSON** and the entire response contract is **one JSON
  object on stdout** with exactly three optional fields (`cancel`, `contextModification`,
  `errorMessage`) — trivially representable in a portable schema, and small enough that a
  lossless round-trip adapter (native JSON in → grim's portable event shape → grim's portable
  response shape → native JSON out) is straightforward.
- Identity is the file path itself (§3) — no config-file entry to splice, which sidesteps the
  entire "preserve every byte outside the managed member" JSON/TOML-splicing problem this
  client's other artifact kinds require.

**Concrete blockers to name:**

1. **Tagged-union input, not a flat schema.** Every event nests its data under a different,
   event-name-derived key (`preToolUse`, `postToolUse`, `taskStart`, …) rather than one
   consistent field layout (§6). A portable schema needs an explicit per-event
   normalization/unwrap step, not a single pass-through.
2. **One physical file per (scope, event) — no native multi-owner story.** Only exactly one
   `PreToolUse` file is ever discovered per directory (§2, §3). If grim wants to let a user
   install **multiple** registry hook packages that each want to react to `PreToolUse`, grim
   itself must be the sole owner of that one file and internally fan out to N registered
   sub-hooks, merging results with the **same algorithm Cline's own `CombinedHookRunner` uses**
   (OR the cancels, `\n\n`-join the context strings, `\n`-join the error strings) to stay
   behaviorally consistent with what a native multi-hook setup would have produced. This also
   means grim must detect and refuse to silently clobber a **pre-existing, hand-written** user
   hook at that path (or offer to fold it in) rather than overwrite it outright.
3. **Two divergent per-platform file conventions that are mutually exclusive.** Unix ignores
   `.ps1`; Windows ignores the extensionless form (§3). grim must materialize the right one (or
   both) per target OS at install time, not a single cross-platform artifact.
4. **A single global kill switch grim doesn't own.** The "Enable Hooks" setting is all-or-
   nothing for the whole client and lives in Cline's own internal state store, not a
   plain file grim can safely edit today (§10) — grim can rely on its default-on behavior but
   cannot itself guarantee the mechanism is active, and has no documented, safe way to flip it
   on programmatically if a user had turned it off.
5. **The contract is a moving target right now.** The vendor is mid-migration between a
   "legacy" bundle (all nine events wired) and a new "SDK bundle" (only six of nine wired as
   of this snapshot — `TaskResume`, `PreCompact`, `Notification` deferred, §4). A trampoline
   for those three would install cleanly but silently do nothing on hosts already migrated to
   the new bundle, through no fault of grim's — this should be re-checked close to any 1.0
   hardening of a portable `hook` artifact kind, since it is explicitly called out in the
   vendor's own source as provisional ("Deferred hooks (NOT wired here)").
6. **No structural input-modification channel.** Unlike the separate (out-of-scope) CLI/SDK
   surface's `overrideInput`, the VS Code extension's `PreToolUse` **cannot** rewrite tool
   parameters before execution (§7) — only observe/log/inject-future-context/cancel. A
   portable schema modeled on this client alone should not promise input-mutation as a
   capability, even though other clients' native hook contracts might offer it.
7. **JetBrains parity is unverified** (§10) — a portable artifact validated only against the
   VS Code source could plausibly not apply, or apply differently, to the JetBrains target
   until directly confirmed.

None of these are fatal to a trampoline design; all are modelable as explicit constraints
(single-owner-per-slot, per-platform file pair, per-event wiring-status flag, no
input-mutation capability) rather than blockers requiring a different architecture.

---

## Sources

| URL | What it establishes | Fetched |
|---|---|---|
| https://docs.cline.bot/ | Root docs nav; no dedicated hooks landing content | 2026-08-14 |
| https://docs.cline.bot/llms.txt | Full doc index; located `features/hooks`, `customization/hooks.md`, `sdk/plugins.md` and siblings | 2026-08-14 |
| https://docs.cline.bot/features/hooks (and `.md` twin, `customization/hooks.md`) | Current hooks doc is a stub redirecting to SDK Plugins page | 2026-08-14 |
| https://cline.bot/blog/cline-v3-36-hooks (mirror: https://cline.ghost.io/cline-v3-36-hooks/) | Official announcement of Hooks: v3.36, 2025-11-06, initial directories/event names/output shape, macOS/Linux-only claim at launch | 2026-08-14 |
| https://docs.cline.bot/sdk/plugins.md | Plugins/runtime-hooks concept, hook points (`beforeRun`, `afterRun`, `beforeTool`, `afterTool`, `beforeModel`, `afterModel`, `onEvent`), policy fields (`mode`, `timeoutMs`, `retries`, `failureMode`) | 2026-08-14 |
| https://docs.cline.bot/sdk/guides/writing-plugins.md | Plugin authoring guidance; "use lifecycle hooks for observation... not for modifying agent behavior" | 2026-08-14 |
| https://docs.cline.bot/customization/plugins.md | End-user plugin install surface; explicit "not applicable on VSCode and JetBrains Extension for now" | 2026-08-14 |
| https://docs.cline.bot/sdk/plugin-install.md | Plugin install methods (file URL/npm/git/local), `.cline/plugins/` paths | 2026-08-14 |
| https://docs.cline.bot/sdk/events.md | Separate, unrelated runtime/session **event subscription** API (`agent.subscribe`) — not a hook mechanism | 2026-08-14 |
| `github.com/cline/cline` repo root listing (`gh api repos/cline/cline/contents/`) | Confirms monorepo layout: `apps/{cli,cline-hub,examples,vscode,vscode-rollout}`, no `apps/jetbrains` | 2026-08-14 |
| `apps/vscode/package.json` (raw, via `gh api`) | Current shipped extension version `4.1.10`, name `claude-dev`, displayName `Cline` | 2026-08-14 |
| `CHANGELOG.md` (repo root, raw via `gh api`) | Full version history text for 3.36.0 → 4.1.10, including every hooks/plugins entry quoted in this report | 2026-08-14 |
| Git tags `v3.36.0`, `v3.68.0`, `v3.70.0`, `v3.71.0`, `v3.72.0`, `v4.0.0` → resolved commit dates via `gh api repos/cline/cline/commits/<sha>` | Exact calendar dates for each version-gate cited in §1 | 2026-08-14 |
| `.clinerules/hooks/README.md` (repo root, raw via `gh api`) | The maintainers' own dogfooded hooks doc: directories, event list ("coming soon" markers now stale), full input/output JSON shapes, merge semantics, troubleshooting, "Security Considerations" section quoted in §9 | 2026-08-14 |
| `apps/vscode/src/core/hooks/templates.ts` | Canonical, current list of all 9 event names + per-event input/output doc comments embedded in generated hook templates; Windows PowerShell template; `Notification` and `PreCompact` full field lists | 2026-08-14 |
| `apps/vscode/src/core/hooks/hook-factory.ts` | `validateHookOutput()` (exact output schema + deprecated `shouldContinue` migration error), `CombinedHookRunner` merge algorithm, `HookFactory` discovery/cwd-resolution logic, fail-open class-doc comment | 2026-08-14 |
| `apps/vscode/src/core/hooks/HookProcess.ts` | Exact `spawn()` invocation for Unix (`shell:true`) and Windows (PowerShell argv), 30s timeout, 1MB output cap, process-group kill | 2026-08-14 |
| `apps/vscode/src/core/hooks/HookDiscoveryCache.ts` | Cache-with-file-watcher-invalidation design — refutes a "snapshot at session start" assumption | 2026-08-14 |
| `apps/vscode/src/core/hooks/HookError.ts` | `HookErrorType` enum (`timeout`, `validation`, `execution`, `cancellation`) and message templates | 2026-08-14 |
| `apps/vscode/src/core/hooks/hooks-utils.ts` | `getHooksEnabledSafe()` — confirms default-enabled (`?? true`) behavior | 2026-08-14 |
| `apps/vscode/src/core/hooks/utils.ts` | `VALID_HOOK_TYPES`, `resolveHooksDirectory()`, `resolveExistingHookPath()` — canonical paths and per-platform filename rules | 2026-08-14 |
| `apps/vscode/src/core/hooks/HookProcessRegistry.ts` | Zombie-process prevention via registry + `terminateAll()` on deactivation | 2026-08-14 |
| `apps/vscode/src/core/hooks/shell-escape.ts` | Exact Unix/Windows shell-path-escaping implementation used before spawning | 2026-08-14 |
| `apps/vscode/src/sdk/hooks-adapter.ts` | **Definitive fail-open proof** (try/catch → `return undefined` on hook failure in `beforeTool`); explicit list of which of the 9 events are wired vs "Deferred" in the new SDK-runtime bundle | 2026-08-14 |
| `sdk/examples/hooks/README.md`, `sdk/examples/hooks/PreToolUse.py` | The **separate, incompatible** CLI/SDK-surface file-hook convention (`.cline/hooks/`, `--hooks-dir`, different output schema with `review`/`context`/`overrideInput`) — included only for disambiguation, out of scope for the assigned VS Code/JetBrains client | 2026-08-14 |
| WebSearch: `Cline "hooks" docs.cline.bot` | Located `docs.cline.bot/features/hooks` and the `cline.ghost.io` blog mirror; summarized "Enable Hooks" settings-UI location and macOS/Linux-only claim (later found stale re: Windows) | 2026-08-14 |
