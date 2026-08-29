# Amp — hook / lifecycle-event mechanism research

Client: **Amp** (Sourcegraph), CLI + editor extensions. Manual: https://ampcode.com/manual
Research date: 2026-08-14. All sources fetched today unless noted.

## Evidence-quality caveat (read first)

Amp's Owner's Manual is a single very long page. The fetch tool used here renders/summarizes
it per-call rather than guaranteeing a full-page grep, so a "zero occurrences" result from one
fetch is not airtight proof of absence — it can be a truncation artifact. Where this matters
(the Toolbox section) I cross-checked against Google's indexed snippets of the same manual URL,
which *do* show toolbox prose in what reads as manual copy ("a toolbox is a directory full of
UNIX-style programs... sits between MCP servers and common CLI tools, but with less complexity
than MCP servers"). I treat Toolbox as currently documented on `/manual` under a "Tools"
subsection, sourced via a mix of direct fetch + search-snippet corroboration — flagged inline
below wherever a claim rests on the weaker (search-snippet) leg.

Where I asked a direct question and the fetch tool affirmatively answered "not mentioned /
NOT DOCUMENTED" after being pointed at the exact section, I record that as `NOT DOCUMENTED`
per the brief's instruction, not as "presumed absent."

---

## 1. Existence & name

Amp has **no feature literally named "hook" or "hooks."** A full-page search for the substring
"hook" (case-insensitive) on `https://ampcode.com/manual` returned zero occurrences (direct
fetch, 2026-08-14).

Three adjacent mechanisms exist; only one of them is a hook by the brief's definition
(client-invoked, deterministic execution of user-supplied code at a lifecycle event):

| Mechanism | Vendor name | Invoked by | In scope as "hook"? |
|---|---|---|---|
| `AMP_TOOLBOX` executables | "Toolbox" / "Bring Your Own Tools" | **the model**, deciding to call a tool | **No** — this is a custom-tool mechanism, not event-invoked. Disambiguating per the brief. |
| `amp.on(event, handler)` in a plugin file | "Plugins" — "Events" | Amp's own lifecycle (session/turn/tool boundaries) | **Yes** — event-invoked, but handler is in-process JS/TS, not an external process |
| `amp.permissions` rule with `"action": "delegate"` | "legacy permissions rules" (Amp's own term) | pre-tool-use, one specific lifecycle moment | **Yes, and the closest match to a classic shell-out hook** — external process, JSON stdin, env vars, exit-code contract |

Versioning: no version number or beta/stable/experimental tag is stated anywhere for the
Plugin system or for `amp.permissions`. Toolbox's introduction is date-anchored by its
announcement post, **"Bring Your Own Tools"** (https://ampcode.com/news/toolboxes),
published **2025-08-29**, extended by **"More Tools for the Agent"**
(https://ampcode.com/news/more-tools-for-the-agent, published **2025-10-22**) which added
support for multiple directories in `AMP_TOOLBOX`. Neither post nor the manual calls Toolbox
experimental/beta — by 2026-08-14 it has been live roughly a year, i.e. de facto stable.

The `amp.permissions` path is explicitly labeled legacy by Amp itself. Manual, verbatim:

> "If Amp detects `amp.permissions`, `amp.guardedFiles.allowlist`, or
> `amp.dangerouslyAllowAll` (set to `false`) in your settings, an internal plugin is activated
> to apply the legacy permissions rules."

i.e. Amp's own forward path for permission policy is a **Plugin** (`tool.call` handler); the
JSON-rules format — including `delegate` — is kept alive only via an internal compatibility
shim. It still fully works today; there is no removal/deprecation date given.

## 2. Config location(s)

**Project scope:** `.amp/settings.json` or `.amp/settings.jsonc` (manual states this is
searched upward from the current working directory — i.e. it need not sit at the repo root).

**Global/user scope:**
- macOS/Linux: `~/.config/amp/settings.json` or `.jsonc`
- Windows: `%USERPROFILE%\.config\amp\settings.json`
- `XDG_CONFIG_HOME` is honored and relocates the `amp/` config root (manual: "`XDG_CONFIG_HOME`
  — determines plugin/skill search paths"; by the same convention this is `$XDG_CONFIG_HOME/amp/`).
- `--settings-file <path>` CLI flag overrides which settings file Amp reads for that invocation.

**Enterprise/managed scope (admin-controlled, overrides individual settings):**
- macOS: `/Library/Application Support/ampcode/managed-settings.json`
- Linux: `/etc/ampcode/managed-settings.json`
- Windows: `%ProgramData%\ampcode\managed-settings.json`

Source: https://ampcode.com/news/enterprise-managed-settings (published **2025-08-25**),
verbatim: "configure organization-wide settings that override individual settings"; it can
"Allow or block MCP servers" and "Allow or block Bash commands that match specified patterns";
"Settings in these files use the same schema as individual settings." The post does **not**
document exact merge semantics (does it deep-merge or replace per-key?) or whether it can
hard-lock a key against override — `NOT DOCUMENTED`.

**Toolbox directory:** named by the `AMP_TOOLBOX` env var — as of the Oct 2025 update this is a
**colon-separated list of directories**, e.g. (from search-indexed manual text):
```
export AMP_TOOLBOX="$PWD/.amp/tools:$HOME/.config/amp/tools"
```
This is a directory-of-executables convention, auto-discovered: "Amp will look in that
directory for executables to be used as tools" (news/toolboxes, verbatim). No merge ambiguity
here — every executable in every listed directory is loaded.

**Plugin directory** — auto-discovered, in this precedence order (manual, "Plugin Locations"):
1. Project: `.amp/plugins/`
2. System: `$XDG_CONFIG_HOME/amp/plugins/` or `~/.config/amp/plugins/`
3. Personal: managed via Amp's web Personal Settings UI
4. Workspace: managed by admins (cloud-side, not a local file)

Entry point shape: a single file `plugin-name.ts`/`.js`, or a directory
`plugin-name/index.ts`/`index.js`. All discovered locations load together (additive), not
"one wins" — multiple plugins can coexist and each independently registers its own event
handlers.

**Skills** (adjacent, not a hook, mentioned only for completeness): `amp.skills.path` setting,
default `"~/my-skills:/shared/team-skills"`-style colon list; also draws from the shared
cross-vendor `$HOME/.agents/skills` pool per grim's own convention (not Amp-specific).

## 3. Config schema — verbatim

### `amp.permissions` (legacy) — an ARRAY, not a named map

Source: `https://ampcode.com/manual/appendix/legacy-permissions-rules.txt` (fetched directly,
2026-08-14). Fields on a rule object: `tool`, `matches`, `action`, `context`, `to`, `message`.

`action` values: `"allow"`, `"reject"`, `"ask"`, `"delegate"`.

Examples (from the appendix, reconstructed field-for-field by the fetch tool):
```json
{ "tool": "Bash", "action": "allow" }
```
```json
{ "tool": "edit_file", "matches": { "path": ".*" }, "action": "reject" }
```
```json
{ "tool": "Grep", "matches": { "path": "$HOME/*" }, "action": "ask" }
```
```json
{
  "tool": "Bash",
  "matches": { "cmd": "gh *" },
  "action": "delegate",
  "to": "my-gh-permission-helper"
}
```
`matches` supports at least `path` (glob/regex-ish string) and `cmd` (shell-style glob, `gh *`
matches a command prefix) sub-fields, tool-dependent. Exact glob-vs-regex grammar for `matches`
values: `NOT DOCUMENTED` (the `.*` and `gh *` examples are ambiguous between the two).

**Stable identity for a third-party installer:** there is **no id/name/description field on a
rule**. A rule is identified only by its full shape (`tool`+`matches`+`action`[+`to`]). An
external installer (grim) wanting to own/update/remove exactly one entry idempotently would
have to either (a) match on a convention it controls entirely (e.g. always use a fixed `to`
value it owns) and diff-replace by that, or (b) maintain an out-of-band manifest of which
array indices/objects it added — the format itself gives no help. This is a genuine gap
against the brief's Q3 ask.

`amp.mcpPermissions` is a sibling array (different key, same shape family) for MCP-server-level
allow/reject, with `matches.command`/`matches.args`/`matches.url` seen in manual examples — same
no-identity-field gap.

### Toolbox — no JSON schema at all; identity = filename

A toolbox "entry" is just an executable file sitting in a directory named by `AMP_TOOLBOX`.
There is no manifest file grim would edit — installing/removing one is literally
adding/deleting a script. This is naturally idempotent for an installer (own the file, own the
lifecycle) but is a **tool**, not an event hook (see §1).

Description self-declared by the script on the `describe` call. One documented shape (from
"Bring Your Own Tools", 2025-08-29, verbatim JSON example):
```json
{
  "name": "run-tests",
  "description": "use this tool instead of Bash to run tests in a workspace",
  "args": { "dir": ["string", "the workspace directory"] }
}
```
A later search-indexed snippet of the manual describes the describe-phase output differently:
"write its description to stdout as a list of key-value pairs, one per line" — I could not get
an independent direct-fetch confirmation of this exact wording or reconcile it with the JSON
example above (possibly the format was simplified/changed between Aug and the current manual,
or the snippet is describing a different, simplified example). Treat the **JSON-object shape as
the primary documented one** (I fetched it directly from the announcement post) and the
key-value-lines description as a lower-confidence secondary claim — `flagged, not resolved`.

### Plugin events — TypeScript, in-process, not a config schema at all

```typescript
import type { PluginAPI } from '@ampcode/plugin'

export default function (amp: PluginAPI) {
  amp.on('tool.call', async (event, ctx) => {
    // event.tool is a string (tool name), confirmed by manual example:
    //   `Allow ${event.tool}?`
    // amp.helpers.shellCommandFromToolCall(event) extracts a shell command
    // when the tool is a shell-executing one (confirmed via the full
    // "Example Plugin: Permissions" code sample, reproduced in §7).
    return { action: 'allow' } // | 'reject-and-continue' | 'modify' | 'synthesize'
  })
}
```
There is no "id" concept here either — identity is the plugin's file path, full stop.

## 4. Event catalogue — verbatim event names (Plugin system only; Toolbox has none)

| Event | Fires when | Handler receives (confirmed fields) |
|---|---|---|
| `session.start` | a thread session begins | `event.thread.id` |
| `agent.start` | the user submits a prompt / new turn begins | `(event, ctx)` — no field confirmed beyond existence |
| `tool.call` | before a tool runs (pre-tool-use gate) | `event.tool` (string name); helper `amp.helpers.shellCommandFromToolCall(event)` |
| `tool.result` | after a tool finishes (post-tool-use) | `event.status`, `event.tool` |
| `agent.end` | the agent finishes its turn | `event.message` |

No events named/found for: file-edit-specific, notification, stop/interrupt (distinct from
`agent.end`), context compaction, subagent-specific, or error. `NOT DOCUMENTED` — either they
don't exist as separate events, or subagent/file-edit activity rides on `tool.call`/
`tool.result` (a subagent invocation and a file edit are both, mechanically, tool calls) —
this is my inference, not a documented statement.

**Schedules** (https://ampcode.com/news/schedule, mentioned in manual) is adjacent but distinct:
agents can self-schedule a future wake-up with a saved prompt ("Check on this backfill job
every ten minutes and ping me on Slack if it stalls."). This is agent-initiated recurring
*execution*, not a client-invoked lifecycle hook — out of scope, noted only to disambiguate.

## 5. Invocation

**Toolbox** — two-phase subprocess exec:
1. Startup: Amp executes every file found under each `AMP_TOOLBOX` directory once, with env var
   `TOOLBOX_ACTION=describe`; script writes its description to stdout.
2. Call time: when the model invokes the tool, Amp **re-executes the same executable** with
   `TOOLBOX_ACTION=execute`, tool-call arguments as JSON on stdin.

Shell command string vs argv array, working directory, `$PATH` handling, timeout, and
concurrency for toolbox executables: **NOT DOCUMENTED** anywhere I could find (announcement
posts and manual are silent on all five).

**Delegate** — subprocess named by the rule's `to` field (bare name, e.g.
`my-gh-permission-helper`; presumably `$PATH`-resolved, not documented explicitly). Timeout,
concurrency, and missing-binary behavior: **NOT DOCUMENTED**.

**Plugin events** — in-process async function call inside Amp's own runtime (Node/Bun-based;
not independently confirmed which). Not a subprocess: no argv, no stdin, no exit code — the
JS return value *is* the response. Ordering/concurrency when two plugins register the same
event: **NOT DOCUMENTED** (I asked the fetch tool directly against the Plugins/Permissions
sections; response: "not mentioned"). What happens if a handler throws: **NOT DOCUMENTED**
(same direct check, same "not mentioned" result).

**A real, documented race condition** relevant to reliability: plugins load asynchronously at
startup. CLI flag `--plugin-ready-timeout [N]` (max 300s) exists because, per the manual,
verbatim: **"Without it, the turn can start before plugins finish loading, and those events are
skipped."** — i.e. the *default* behavior is fail-open/silent-skip for early-session plugin
events, not block-and-wait.

## 6. Input payload — verbatim

- **Toolbox execute phase:** JSON object of tool-call arguments on stdin. Announcement post's
  own paraphrase of the read pattern: `JSON.parse(fs.readFileSync(0, 'utf-8'))['dir']`.
- **Delegate:** tool parameters as JSON on stdin, **plus** three env vars:
  `AMP_THREAD_ID`, `AGENT_TOOL_NAME` (set to the invoked tool's name), `AGENT=amp`.
- **Plugin `tool.call`:** `event.tool` (string). Full input-argument field name (e.g.
  `event.input`/`event.args`) not confirmed verbatim in any fetched text — the only proven way
  to reach the underlying command is the `amp.helpers.shellCommandFromToolCall(event)` helper,
  which implies the raw arguments live somewhere on `event` but the exact key is
  **NOT DOCUMENTED** in what I could retrieve.
- **CLI `--stream-json` / `--stream-json-input`** (adjacent observability channel, not a hook —
  included because the brief asked about it explicitly): NDJSON, one JSON object per line.
  Manual, verbatim: **"Amp's stream JSON output tries to be compatible with Claude Code's
  format as much as possible."** Confirmed line shapes (fetched from
  `https://ampcode.com/manual/appendix#stream-json-output`, 2026-08-14):
  ```json
  {"type":"user","message":{"role":"user","content":[{"type":"text","text":"what is 3 + 5?"}]},"parent_tool_use_id":null,"session_id":"T-..."}
  {"type":"assistant","message":{"type":"message","role":"assistant","content":[{"type":"text","text":"8"}],"stop_reason":"end_turn","usage":{"input_tokens":10}},"parent_tool_use_id":null,"session_id":"T-..."}
  {"type":"system","subtype":"init","cwd":"/Users/orb","session_id":"T-...","tools":["Bash","finder"],"mcp_servers":[]}
  {"type":"result","subtype":"success","duration_ms":5400,"is_error":false,"num_turns":1,"result":"8","session_id":"T-..."}
  ```
  Content-block-level types inside `assistant` messages: `tool_use` (`{"type":"tool_use","id":"toolu_...","name":"read","input":{"path":"..."}}`), `tool_result`
  (`{"type":"tool_result","tool_use_id":"toolu_...","content":"[...]","is_error":false}`), and
  `thinking`/`redacted_thinking` (only with `--stream-json-thinking`). For `--stream-json-input`,
  a queued/interrupting message sets `"steer": true` (manual, paraphrased: set `steer` to `true`
  to mark a message as steering if it's queued while the agent is busy).

## 7. Output / response contract — verbatim

**Delegate — the cleanest, fully-specified contract found in this research:**
- Exit code `0` → **allow**
- Exit code `1` → **ask** the operator (falls back to an interactive prompt)
- Exit code `≥2` → **reject**, and the process's **stderr is surfaced to the model** as the
  rejection reason.
- No JSON response object, no way to modify the tool call or inject extra context on allow —
  purely a three-way gate. stdout content on the allow/ask paths: **NOT DOCUMENTED**.

**Plugin `tool.call` — JS object return value**, action field one of:
- `'allow'`
- `'reject-and-continue'` — takes a `message` string, shown back into the conversation as the
  reason (confirmed via the full permissions-plugin example, reproduced below)
- `'modify'` — replaces the tool call's input (exact replacement field name beyond "input" not
  independently confirmed)
- `'synthesize'` — fabricates a tool result without running the tool (exact output field name
  not independently confirmed beyond "output")

**Full verbatim code sample** ("Example Plugin: Permissions", fetched directly from the manual,
2026-08-14) — the most concrete evidence of the real `event`/`ctx` surface:
```typescript
import type { PluginAIAskResult, PluginAPI } from '@ampcode/plugin'

export default function (amp: PluginAPI) {
	const safePatterns = [
		/^\s*git\s+status\b/, /^\s*git\s+log\b/, /^\s*git\s+diff\b/, /^\s*git\s+show\b/,
		/^\s*git\s+branch\s*$/, /^\s*git\s+branch\s+-[av]\b/, /^\s*git\s+stash\s+list\b/,
		/^\s*git\s+remote\s+-v\b/, /^\s*git\s+fetch\b/, /^\s*git\s+pull\b/,
		/^\s*git\s+add\b/, /^\s*git\s+commit\b/, /^\s*git\s+push\b(?!.*(-f|--force))/,
	]

	amp.on('tool.call', async (event, ctx) => {
		const shellCommand = amp.helpers.shellCommandFromToolCall(event)
		if (!shellCommand?.command) return { action: 'allow' }
		const command = shellCommand.command
		if (!/^\s*git\s+/.test(command)) return { action: 'allow' }
		if (safePatterns.some((pattern) => pattern.test(command))) return { action: 'allow' }

		const aiResponse: PluginAIAskResult = await amp.ai.ask(
			`Does this git command look like a potentially destructive operation ...? Command: ${command}`,
		)
		if (aiResponse.result === 'no') return { action: 'allow' }

		const confirmed = await ctx.ui.confirm({
			title: 'Potentially destructive git operation',
			message: `${command}\n\nReason: ${aiResponse.reason}\n\nDo you want to proceed?`,
			confirmButtonText: 'Allow',
		})
		if (confirmed) return { action: 'allow' }
		return {
			action: 'reject-and-continue',
			message: `User cancelled potentially destructive git operation: ${command}`,
		}
	})
}
```
Note `amp.ai.ask(...)` — plugins can call back into an LLM classifier
(`{ result: 'yes'|'no'|'unknown', reason: string }`) as part of a hook decision; and
`ctx.ui.confirm(...)` can pop an interactive prompt from inside a hook. Both are only possible
because the handler runs in-process — an external-process trampoline could not replicate them.

`agent.end` — verbatim manual paraphrase: "Return `continue` to append follow-up message and
start another turn," i.e. `{ action: 'continue', userMessage: '...' }`.

Where does output go, to user/model/both? For delegate: stderr → model (on reject), confirmed.
For plugin actions: `message`/`userMessage` strings clearly reach the conversation (model and
transcript); whether they also render distinctly in the user-facing UI is **NOT DOCUMENTED**
beyond the obvious (a `reject-and-continue` message would need to be visible somewhere for the
user to understand why a command didn't run).

## 8. Reliability & limits

- Plugin load is async at session/CLI start; default is **fail-open with silent event-skipping**
  if the first turn starts before plugins finish loading (see §5's `--plugin-ready-timeout`
  quote). Max override: 300s.
- Plugins are **not hot-reloaded** on file change. Manual, verbatim: **"After changing a plugin,
  ask Amp to reload it."** Mechanisms: ask Amp in-conversation, or open the command palette
  (`Ctrl+O`) and run `plugins: reload`, or `amp plugins list` to inspect what's currently loaded.
  This is precisely the "config snapshotted at startup" gotcha the brief asked about — confirmed
  for plugins specifically.
- Handler-exception behavior (fail open vs fail closed) and multi-plugin-same-event ordering/
  conflict resolution: **NOT DOCUMENTED** (directly checked, fetch tool reported "not mentioned"
  against both the Plugins and Permissions manual sections).
- Toolbox timeout, non-zero-exit handling, missing-binary handling, and parallelism:
  **NOT DOCUMENTED**.
- Delegate timeout and missing-binary handling: **NOT DOCUMENTED**.
- Blocking vs fire-and-forget: not explicitly stated for any of the three mechanisms, but
  blocking is the only sensible inference for `tool.call`/`delegate`/toolbox-execute since the
  agent needs a return value before it can proceed — presented here as inference, not a quoted
  fact.

## 9. Security posture

Manual, verbatim, under Permissions: **"Amp does not ask for approval before running tools."**
This is the explicit default — no confirmation prompts, no allowlist gate, unless the operator
opts into `amp.permissions`/a permissions plugin/managed settings.

Security reference (`https://ampcode.com/security`, fetched 2026-08-14), under **"Prompt
Injection Defenses,"** lists **"delegating decisions to external policy helpers"** as one of
several defenses (alongside frontier-model judgment, the permissions system generally, web
context via "Parallel," automatic secret redaction, thread audit trails, and retention
controls). This is significant: Amp frames the `delegate` mechanism as a recognized part of its
own security architecture, not an obscure escape hatch — reinforcing that it's safe to build on
even though it's labeled "legacy" in the settings-schema sense.

Enterprise managed-settings (`/etc/ampcode/managed-settings.json` etc.) exists specifically so
an org can override individual users' permissive defaults: "Allow or block MCP servers" and
"Allow or block Bash commands that match specified patterns" org-wide.

I did **not** find any explicit "hooks/plugins/toolbox scripts are arbitrary code execution, be
careful" warning comparable to some other clients' docs — checked both `/security` and the
manual's Permissions/Plugins sections directly. Likewise **no evidence of a trust-on-first-run
prompt** ("this repo defines a plugin — allow it to load?") for `.amp/plugins/` or
`.amp/tools/` content newly appearing in a freshly cloned repo — `NOT DOCUMENTED` either way;
given plugins load automatically by directory convention with no approval step mentioned
anywhere, the working assumption should be that **cloning a repo and opening it in Amp is
sufficient to execute its `.amp/plugins/*.ts` and any `AMP_TOOLBOX`-listed executables**, which
is a meaningfully more permissive posture than clients that gate this behind an explicit
first-run trust prompt.

## 10. Third-party installability

All three mechanisms are plain file edits, realistically installable by an external tool:
- Toolbox: drop an executable under a directory listed in `AMP_TOOLBOX` (commonly `.amp/tools/`
  for project scope by convention, though `AMP_TOOLBOX` itself must still be set/exported —
  **NOT DOCUMENTED** whether Amp auto-adds a project's `.amp/tools/` to the toolbox path without
  the env var being explicitly set; the examples always show the user exporting it).
- Plugin: drop a `.ts`/`.js` file/dir under `.amp/plugins/` (project) or
  `~/.config/amp/plugins/` (global) — auto-discovered, no registration step needed beyond the
  file existing.
- Delegate rule: splice an object into the `amp.permissions` array in `.amp/settings.json` /
  `~/.config/amp/settings.json` — this is exactly the kind of "splice into existing native JSON,
  preserve everything else" edit grim already does for other clients' configs.

Vendor's own scaffolding commands exist alongside (`amp tools make`, `amp plugins add <url>`,
`amp skill add <source>`) but nothing suggests direct file editing is disallowed or bypassed by
these — they appear to be convenience wrappers over the same files.

**The concrete "needs a restart/reload" gotcha:** confirmed for plugins (§8) — a plugin
installed while Amp is already running a session will not take effect until `plugins: reload`
or a restart. Not confirmed either way for `AMP_TOOLBOX` executables or for
`amp.permissions` changes to `.amp/settings.json` — the toolbox announcement's "on start, Amp
will invoke each executable" phrasing suggests a similar startup-time scan, but this is an
inference, not a documented statement about mid-session file additions.

## 11. Trampoline viability

**Best candidate: the `delegate` permission action.** It is already shaped exactly like a
generic external-command trampoline target: JSON-on-stdin, three env vars, three-way exit-code
contract. A `grim hook run --client amp --event pre-tool-use` command could sit directly at the
`to` position of a generated `delegate` rule with essentially no adaptation layer.

Blockers, concretely:
1. **Single-event coverage.** `delegate` only exists for the pre-tool-use permission moment.
   There is no `delegate`-equivalent for `session.start`, `agent.start`, `tool.result`, or
   `agent.end` — those five events exist **only** on the in-process Plugin API.
2. **"Legacy" labeling.** Amp's own docs steer new configuration toward writing a Plugin
   instead; whether `amp.permissions`/`delegate` will keep working indefinitely is not
   guaranteed by anything I found (no removal date given, but also no explicit permanence
   promise).
3. **Coarser contract than other clients' native hooks.** No JSON response object, no
   `modify`/`synthesize`-equivalent, no way to inject extra context back to the model on allow
   — only allow/ask/reject+stderr-string. A portable hook schema designed against Claude Code's
   richer JSON response contract would have to degrade gracefully to this three-way gate for Amp.
4. **Full lifecycle coverage requires shipping actual JS/TS, not config.** To cover
   `session.start`/`agent.start`/`tool.result`/`agent.end`, grim would have to generate a real
   `.amp/plugins/<name>.ts` (or `.js`) file whose handler body shells out synchronously to
   `grim hook run --client amp --event <E>`, JSON-encodes the event on that subprocess's stdin,
   and maps the child's stdout/exit code back into the specific JS return shape each event
   expects (`{action:...}` for `tool.call`, `{action:'continue', userMessage}` for `agent.end`,
   etc.) — a real code-generation problem, not a config-splice problem, and still subject to the
   no-hot-reload gotcha (§8/§10) after every install/update.
5. **Toolbox is out of scope entirely** — it cannot fire on a lifecycle event under any framing;
   it only exists to add a model-invokable tool.

Net: a trampoline is **plausible for pre-tool-use only** via `delegate` with near-zero
adaptation, and **possible but code-generation-heavy** for the other four lifecycle points via a
generated plugin shim. There is no single mechanism that covers all five events with a config
edit alone.

## Sources

| URL | What it establishes | Fetched |
|---|---|---|
| https://ampcode.com/manual | ToC, settings.json key list, CLI flags, env vars, config paths, plugin locations/precedence, plugin event names, "Amp does not ask for approval before running tools," legacy-permissions activation note, "amp tools list" under a Tools heading, `--plugin-ready-timeout` race-condition note, plugin manual-reload requirement, full "Example Plugin: Permissions" code sample | 2026-08-14 |
| https://ampcode.com/manual/appendix/legacy-permissions-rules.txt | `amp.permissions` rule schema (fields, action values incl. `delegate`), `delegate`'s stdin/env-var/exit-code contract, example rules | 2026-08-14 |
| https://ampcode.com/manual/appendix and `#stream-json-output` | Stream-JSON is "Claude Code compatible"; NDJSON line shapes for `user`/`assistant`/`system`/`result`, content-block types `tool_use`/`tool_result`/`thinking`, `steer` field | 2026-08-14 |
| https://ampcode.com/security | "Amp does not ask for approval before running tools" (corroborating), "delegating decisions to external policy helpers" under Prompt Injection Defenses, secret redaction, no explicit hooks-are-arbitrary-code-execution warning found | 2026-08-14 |
| https://ampcode.com/news/toolboxes ("Bring Your Own Tools") | `AMP_TOOLBOX`/`TOOLBOX_ACTION` two-phase describe/execute contract, JSON describe-output example, published 2025-08-29 | 2026-08-14 |
| https://ampcode.com/news/more-tools-for-the-agent | Multiple `AMP_TOOLBOX` directories (colon-separated) added, published 2025-10-22 | 2026-08-14 |
| https://ampcode.com/news/enterprise-managed-settings | `managed-settings.json` paths and org-override behavior, published 2025-08-25 | 2026-08-14 |
| https://ampcode.com/news/streaming-json | Streaming JSON feature intro, published 2025-09-09 | 2026-08-14 |
| Google search snippets of `ampcode.com/manual` (via WebSearch) `[unofficial-corroboration]` | "a toolbox is a directory full of UNIX-style programs... sits between MCP servers and common CLI tools," "amp tools make," describe-phase "key-value pairs, one per line" phrasing — used only as corroboration/leads where a direct WebFetch pass didn't independently surface the same text; not treated as sole evidence for any claim above | 2026-08-14 |
