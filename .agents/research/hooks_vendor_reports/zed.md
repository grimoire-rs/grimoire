# Zed — native hook / lifecycle-event mechanism

Researched 2026-08-14. Client: **Zed** (zed.dev / zed-industries/zed). Scope per brief:
client-invoked, deterministic execution of *user-supplied code* at agent lifecycle/tool
events — as distinct from slash commands, skills, subagents, MCP servers, rules files,
custom tools, LSP/formatter integration, and git hooks.

## Bottom line

Zed's **first-party Agent Panel has no hook / lifecycle-event mechanism today.** This is
not an inference from silence alone — it is corroborated by (a) the extension API's
method list, which has no agent-lifecycle callback, (b) four separate community
issues/discussions asking for exactly this capability, none shipped, and (c) the one
feature in Zed that *is* called "hooks" (on Task templates) is scoped to a single
non-agent editor event and cannot observe or influence the agent at all.

---

## 1. Existence & name

**Does not exist** for the Agent Panel / built-in agent. `NOT DOCUMENTED` as a shipped
feature anywhere in official docs (`zed.dev/docs/ai/*`), the settings reference, or the
extension API.

What *does* exist and is easily confused with it:

- **Task hooks** (real, shipped, editor-scoped, not agent-scoped) — a `hooks` array field
  on a Task template. As of this research there is exactly **one** hook kind:
  `create_worktree` (fires after Zed creates a linked git worktree). Source: Rust struct in
  `crates/task/src/task_template.rs` (fetched via raw GitHub 2026-08-14):

  ```rust
  #[derive(Clone, Copy, Debug, PartialEq, Hash, Eq, Serialize, Deserialize, JsonSchema)]
  #[serde(rename_all = "snake_case")]
  pub enum TaskHook {
      #[serde(alias = "create_git_worktree")]
      CreateWorktree,
  }
  ```
  and `pub hooks: HashSet<TaskHook>` on the task template. Docs at `zed.dev/docs/tasks`
  (fetched 2026-08-14) confirm: "tasks can be configured to run automatically in response
  to certain Zed events by adding a hook to the `hooks` field on a task template," with
  `create_worktree` the only documented value, exposing `ZED_WORKTREE_ROOT` and
  `ZED_MAIN_GIT_WORKTREE` env vars to the spawned task. This is a **generic-editor**
  hook, not an AI-agent hook — it cannot see or affect a model turn, a tool call, or a
  chat message. Mentioned only to disambiguate per the brief's scope note.

- **Notification toggles** (real, shipped, not user-code) — `agent.notify_when_agent_waiting`
  and `agent.play_sound_when_agent_done` settings keys. Docs (`zed.dev/docs/ai/agent-panel`,
  fetched 2026-08-14): "If you send a prompt to the Agent and then put Zed in the
  background, you can choose to be notified when its generation wraps up via: a visual
  desktop notification from your operating system [or] a sound notification... you can use
  the `agent.notify_when_agent_waiting` and `agent.play_sound_when_agent_done` settings keys
  to customize that, including turning both off entirely." These are **booleans that toggle
  Zed's own built-in OS notification/sound** — there is no field to point them at a custom
  command or script. This is the "notify" analogue other clients expose as a real hook
  (e.g. opencode's `notify` command); Zed's version is not extensible.

- **Tool Permissions** (real, shipped, closest thing to a "policy hook", still not user-code
  at an event) — `zed.dev/docs/ai/tool-permissions` (fetched 2026-08-14) documents
  per-tool `always_allow` / `always_deny` / `always_confirm` pattern lists plus
  "Built-in security rules: Hardcoded protections (e.g., `rm -rf /`). Cannot be overridden."
  This governs whether the agent is *allowed* to run a tool call at all; it has no
  mechanism to shell out to arbitrary user-supplied code as part of the decision (no
  "run this command and gate on its exit code" primitive, unlike Claude Code's
  `PreToolUse`/permission-decision hooks).

### Proposals (not implemented — labelled explicitly)

Four Zed community items describe wanting exactly the artifact kind grim is designing.
None has shipped as of 2026-08-14:

| # | Type | Title | State | Created | Updated |
|---|---|---|---|---|---|
| [#57890](https://github.com/zed-industries/zed/issues/57890) | Issue | "Feature: AI Agent extensibility — Custom Commands, Lifecycle Hooks, and Skills" | **CLOSED** | 2026-05-28 | 2026-05-28 |
| [#57943](https://github.com/zed-industries/zed/discussions/57943) | Discussion (companion to #57890) | same title | **OPEN**, unanswered | 2026-05-28 | 2026-06-18 |
| [#52688](https://github.com/zed-industries/zed/issues/52688) | Issue | "Feature request: Tool-use hooks/events for the built-in agent" | **CLOSED** | 2026-03-29 | 2026-04-04 |
| [#34962](https://github.com/zed-industries/zed/discussions/34962) | Discussion | "Hooks 🪝 to execute predefined Agent tasks - From Kiro IDE" | **OPEN**, unanswered | 2025-07-23 | 2025-08-08 |
| [#8325](https://github.com/zed-industries/zed/issues/8325) | Issue | "Editor hooks that can be configured with tasks" | **OPEN** | 2025-01-02 (approx, first comment same day) | ongoing, comments through 2026 |

Issue #57890 / discussion #57943 propose the exact shape grim is evaluating — quoted
verbatim from the issue body (fetched 2026-08-14) because the *shape itself* is
informative even though it is only a proposal:

> **Proposed hook points**
> | Hook | When it fires |
> |------|---------------|
> | `session_start` | When a new agent thread is created |
> | `pre_tool_use` | Before the agent invokes a tool (edit, terminal, search, etc.) |
> | `post_tool_use` | After a tool invocation completes |
> | `generation_end` | After the agent finishes generating a response |
> | `tool_permission_denied` | When the user denies a tool permission |
>
> **Example configuration (in `settings.json`):**
> ```json
> {
>   "agent.hooks": {
>     "session_start": { "command": "echo 'Session started at $(date)' >> .zed/agent-log.txt" },
>     "pre_tool_use": { "command": "echo 'Tool: $ZED_TOOL_NAME File: $ZED_FILE_PATH' >> .zed/agent-log.txt" },
>     "post_tool_use": { "command": "~/.scripts/track-ai-edits.sh" }
>   }
> }
> ```

This is **speculative community text, not a vendor commitment** — issue #57890 is closed
(Zed's bot triage-closes many feature requests; no maintainer design commitment is
recorded in the comment thread we fetched) and discussion #57943 has zero maintainer
replies as of 2026-08-18 last-update. Treat `agent.hooks` / `session_start` /
`pre_tool_use` / `post_tool_use` / `generation_end` / `tool_permission_denied` as **names
a proposal invented**, not a contract that exists.

Issue #52688 (closed) is the sharpest evidence of absence — filed by the maintainer of
the [git-ai](https://github.com/git-ai-project/git-ai) attribution tool, explicitly
because they hit the missing capability building real third-party tooling:

> "When the built-in Zed Agent uses tools (e.g. file edits, terminal commands), there's
> currently no way for extensions or external processes to receive notifications
> before/after those tool invocations." ... "But for Zed's first-party agent, there are no
> hooks or events to observe tool use."

It proposes `agent/tool_use_start` / `agent/tool_use_end` events, explicitly modeled on
"Claude Code's `PreToolUse` / `PostToolUse` hooks," to be exposed as either an Extension
API callback, an ACP notification, or "even simple shell hooks (like git hooks)" — i.e.
the filer considered three plausible integration shapes and got none.

Discussion #34962 (open, unanswered) proposes `.zed/hooks.json` explicitly modeled on
Kiro's hook system (`kiro.dev/docs/hooks`), triggered on IDE events like `git:pre-commit`,
running an **agent prompt** (not a shell command) as the hook body — a different flavor
(prompt-triggering) than #57890's shell-command flavor. Two competing proposal shapes
existing in parallel is itself evidence nothing has converged into a real spec yet.

Issue #8325 (open since 2025-01-02, general "editor hooks via tasks") has accumulated
comments through 2026 explicitly asking for the *agent* case to be folded in:
- 2026-02-18, `@TylorS`: "I'd love to see this expanded to agent hooks similar to
  https://cursor.com/docs/agent/hooks"
- 2026-03-13, `@dlight`: "Agent hooks like this cursor one or [Claude Code's] from claude
  code would unlock #45334 as well... there's a discussion for agent hooks now,
  https://github.com/zed-industries/zed/discussions/57943"

Net: the community is actively pointing at Cursor's and Claude Code's shipped hook
systems as the model to copy, which is a signal Zed maintainers have not yet acted on.

## 2. Config location(s)

N/A — no agent-hook config exists. For the adjacent real mechanisms:

- **Settings** (where `agent.hooks` would live if it existed, per the proposal): project
  `.zed/settings.json`; global per the requesting agent's brief: `$XDG_CONFIG_HOME/zed/settings.json`
  on Linux/FreeBSD, `~/.config/zed/settings.json` on macOS, `%APPDATA%\Zed\settings.json`
  on Windows. JSON with comments (JSONC) — Zed's settings support `//` comments per its
  own schema tooling. Accessible via command palette `zed: open settings file` /
  `agent: open settings`.
- **Tasks** (the real `hooks` field): global `~/.config/zed/tasks.json` (via `zed: open
  tasks`), project `.zed/tasks.json` (via `zed: open project tasks`), plus ephemeral
  "oneshot" tasks created in the spawn modal. `zed.dev/docs/tasks` (fetched 2026-08-14)
  does not state whether project and global task lists merge or one wins; **NOT
  DOCUMENTED** in the fetched content — treat as unconfirmed rather than assume either
  merge or override.
- **MCP servers** live under `context_servers` at the top level of `settings.json` (both
  scopes), confirmed via `zed.dev/docs/ai/mcp` (fetched 2026-08-14):
  ```json
  { "context_servers": { "server-name": { "command": "some-command", "args": ["arg-1"], "env": {} } } }
  ```
  and a remote/URL variant with `headers`. Not a hook surface, listed only because the
  task brief named it as a known grim-relevant path.
- **Extensions** (Rust/WASM) install under `~/Library/Application Support/Zed/extensions/installed/`
  (macOS), `~/.local/share/zed/extensions/installed` (Linux), `$env:LOCALAPPDATA\Zed\extensions\installed`
  (Windows) — per `zed.dev/docs/configuring-zed` (fetched 2026-08-14). No agent-lifecycle
  extension point exists to install into regardless (see §4/§11).

## 3. Config schema — verbatim

N/A — nothing shipped. The only real "hooks" schema in the product is the Task one:

```rust
pub hooks: HashSet<TaskHook>,   // TaskTemplate field

#[serde(rename_all = "snake_case")]
pub enum TaskHook {
    #[serde(alias = "create_git_worktree")]
    CreateWorktree,
}
```
— i.e. a **named-set of hook-kind strings attached to a task entry** (`"hooks": ["create_worktree"]`),
not a map keyed by event with a handler body, and not a general array of independent hook
objects. A task with this hook has no matcher/filter syntax (there is exactly one kind to
match), and the task's own `label` doubles as a human identity — but since only one hook
kind exists and it isn't agent-related, this schema shape cannot be repurposed for grim's
portable hook artifact.

The **proposed** (unshipped) `agent.hooks` shape from #57890 is a named map,
`{"agent.hooks": {"<event_name>": {"command": "<shell string>"}}}` — flat, one handler per
event name, command as a plain shell string with `$ZED_*`-style env var interpolation
(mirrors Zed's existing Task variable-interpolation convention). No id/name/description
field, no matcher/filter concept, because it never got past the proposal stage.

## 4. Event catalogue

**None exist for the agent.** The proposal's event names — `session_start`,
`pre_tool_use`, `post_tool_use`, `generation_end`, `tool_permission_denied` — and
#52688's alternative — `agent/tool_use_start`, `agent/tool_use_end` — are unshipped
community-proposed names, not a contract. Do not treat either list as authoritative.

Real, shipped, non-agent events Zed exposes:
- `create_worktree` (Task hook, §1/§3) — fires after a linked git worktree is created.
- Implicit "agent generation finished" / "agent waiting for input" moments drive the
  notify toggles (§1), but these are **not named, addressable events** — they're wired
  directly to a boolean-gated OS notification call in Zed's own code, not to a
  general event bus a third party can subscribe a command to.

## 5. Invocation

N/A for agent hooks (none exist). For Task hooks, the invocation model documented at
`zed.dev/docs/tasks` matches normal task execution: the task's `command`/`args` run in a
new or reused terminal (`use_new_terminal`), in `cwd` (defaults to task-appropriate
directory), via `shell` (`"system"` or an explicit program+args), with `env` merged in.
This opens a **visible terminal panel** — it is not a headless/backgrounded execution
model, which by itself would make it a poor fit for a synchronous agent-blocking hook
(e.g. a `PreToolUse`-style deny gate) even if it were wired to agent events, since nothing
about the Task runner is documented as blocking the caller on completion or exit code.

## 6. Input payload — verbatim

N/A for agent hooks. Task hooks receive data purely through **environment variables /
`$VAR` template interpolation** in the command string — e.g. `$ZED_WORKTREE_ROOT`,
`$ZED_MAIN_GIT_WORKTREE` for `create_worktree`; general tasks also get `$ZED_FILE`,
`$ZED_SELECTED_TEXT`, `$ZED_GIT_SHA`, `$ZED_GIT_REF`, etc. (`zed.dev/docs/tasks`, fetched
2026-08-14). There is **no JSON-on-stdin contract anywhere in Zed's task or agent
surface** — no example of a hook receiving a structured payload object exists in the
docs we could reach.

## 7. Output / response contract — verbatim

N/A for agent hooks — there is nothing to report a contract for. Task hook exit codes
feed only the terminal UI conventions already used for manually-run tasks (`reveal`:
`always`/`no_focus`/`never`, `hide`: `never`/`always`/`on_success`) — i.e. exit status
controls whether the terminal panel shows/hides itself, not whether anything is
allowed/denied/injected into a model context. No JSON-response-object parsing of stdout
exists anywhere in the fetched docs; nothing analogous to Claude Code's exit-code-2 deny
semantics or a `{"decision": "block", ...}` object was found for any Zed surface.

## 8. Reliability & limits

N/A for agent hooks. For Task hooks: no documented timeout (`NOT DOCUMENTED`);
`allow_concurrent_runs` is a per-task boolean but its interaction with the
`create_worktree` hook specifically isn't spelled out; ordering/blocking semantics
relative to the worktree-creation flow that triggers it are **NOT DOCUMENTED** in the
pages fetched — we did not find text stating whether Zed waits for the hook task to
finish before continuing, or fires it and moves on.

## 9. Security posture

Zed's actual arbitrary-code-execution control plane — which would be the natural place a
hook-approval gate lives if hooks existed — is **Tool Permissions**
(`zed.dev/docs/ai/tool-permissions`, fetched 2026-08-14), governing what the *agent
itself* may do, not third-party hook scripts:

> "When the agent requests permission, you'll see in the thread view a tool card with a
> menu that includes: Allow once / Deny once" [with broader always-allow/always-deny
> pattern rules also available], and: "Built-in security rules: Hardcoded protections
> (e.g., `rm -rf /`). Cannot be overridden."

Settings shape: `agent.tool_permissions.default` (`"confirm"` / `"allow"` / `"deny"`),
plus per-tool `always_allow` / `always_deny` / `always_confirm` pattern lists. Zed's
August 2026 stable release (per release-notes search, fetched 2026-08-14) added
**sandboxing for the Agent's terminal commands and web fetches** — again a containment
mechanism for the agent's own actions, unrelated to any third-party hook trust model,
since no hook surface exists to need one. No docs page anywhere warns about
"hooks execute arbitrary code" for the simple reason that Zed has no hook feature to
warn about; this is the strongest structural evidence of absence, since every
comparable client we're aware of that *does* ship hooks (Claude Code, Cursor, per the
community comments quoted in §1) prominently documents that exact warning.

Separately and out of scope per the brief (mentioned only to avoid ambiguity): the
August 2026 release also added a **"Skip Hooks"** toggle in the Git Panel commit
flow, and a fix for "commit-msg hooks being silently skipped on commit" — these are
**git hooks** (pre-commit/commit-msg), not agent hooks; Zed is just respecting/allowing
skip of the repository's own Git hook mechanism when committing through its UI.

## 10. Third-party installability

Moot for agent hooks (nothing to install). For the mechanisms that do exist and that
grim already targets on other clients: `context_servers` (MCP) and `settings.json` in
general are plain JSON/JSONC files a third-party tool can splice in place, matching how
grim already handles other Zed surfaces per this task's brief. Whether Zed hot-reloads
`settings.json` / `tasks.json` on external edit without a restart is **NOT DOCUMENTED**
in the pages fetched for this research — not confirmed either way, so do not assume
grim-installed changes take effect without user action until verified against Zed's
settings-file-watching behavior directly.

## 11. Trampoline viability

**Not viable today for Zed's first-party agent** — there is no event to trampoline into.
A single generic `grim hook run --client zed --event <E>` command has no native contract
to be invoked by, because:
- The Extension (Rust/WASM) API's `Extension` trait exposes no agent-lifecycle method —
  confirmed by enumerating its methods (language servers, slash commands, context
  servers/MCP, doc-indexing, debug adapters only; `docs.rs/zed_extension_api`, fetched
  2026-08-14). Even if grim were willing to ship a compiled WASM extension (a much heavier
  ask than writing a JSON file), there is no callback to attach to.
- Task hooks are a closed enum with one non-agent variant (`create_worktree`); grim
  cannot add new hook kinds by editing `tasks.json` — the kind has to exist in Zed's own
  Rust binary first.
- The notify toggles and tool-permission rules are booleans/pattern-lists with no command
  field — nothing to point at `grim hook run`.

**One theoretical, undocumented, and fragile path exists — via ACP, and only for
externally-hosted agents, not Zed's own agent.** `agent_servers` in `settings.json`
launches an ACP-speaking subprocess by command (`{"agent_servers": {"my-agent":
{"type": "custom", "command": "node", "args": ["...", "--acp"]}}}`,
`zed.dev/docs/ai/external-agents`, fetched 2026-08-14). Because that command is
user-specified, a third party could in principle point it at a **wrapper** that
sits between Zed and the real agent binary, passing through the ACP JSON-RPC stream
(`session/update` notifications, `session/request_permission` calls — confirmed via
`agentclientprotocol.com` and its Rust SDK docs, fetched 2026-08-14) while also
running side-effects on the intercepted messages. This is exactly what the git-ai
project's own issue (#52688) says it already does: "For external agents launched via
ACP `agent_servers`, we can intercept tool calls through a JSON-RPC proxy." But:
  - This is a **third party's own proxy engineering**, not a documented or sanctioned
    "hook" contract ACP exposes — ACP itself defines a structured client/agent RPC
    protocol (session updates, permission requests, tool-call reporting), not a
    plugin/middleware/hook registration point. Nothing in the ACP schema docs
    (`agentclientprotocol.com/protocol/schema`) describes third-party interception as
    a supported extension mechanism.
  - It only ever applies to **agents Zed hosts over ACP** (e.g. Claude Code, Gemini
    CLI, or any custom ACP server a user points `agent_servers` at) — it says nothing
    about Zed's own built-in first-party Agent Panel assistant, which doesn't go
    through ACP internally and has no exposed RPC stream to intercept.

**This last point is the one that matters most for grim's design**: when a user runs
Claude Code (or another hook-capable agent) *inside* Zed via `agent_servers`/ACP, that
hosted agent is a normal separate process reading its **own native config from disk** —
e.g. Claude Code's own `.claude/settings.json` hooks apply exactly as they would running
Claude Code standalone in a terminal, completely independent of Zed. **Grim should keep
targeting the hosted agent's own native config for that case** (already covered by the
sibling per-client research in this task set), not invent a Zed-specific hook target for
it. Zed itself only becomes the right target for the narrow case of the built-in Agent
Panel — and for that case, there is currently nothing to target.

## Sources

| URL | What it establishes | Fetched |
|---|---|---|
| https://zed.dev/docs/ai/overview | AI docs index; no hook/lifecycle/automation/notification mechanism described | 2026-08-14 |
| https://zed.dev/docs/ai/agent-panel | `agent.notify_when_agent_waiting` / `agent.play_sound_when_agent_done` are boolean OS-notification/sound toggles only, no custom-command field | 2026-08-14 |
| https://zed.dev/docs/ai/agent-settings | Agent settings keys (models, compaction, subagent model, etc.) — no hooks/automation keys among them | 2026-08-14 |
| https://zed.dev/docs/ai/tool-permissions | Tool permission menu (Allow once/Deny once, always_allow/always_deny patterns), hardcoded `rm -rf /` protection quote | 2026-08-14 |
| https://zed.dev/docs/ai/external-agents | `agent_servers` settings shape for custom ACP agents; ACP boundary between Zed config and agent's own native config | 2026-08-14 |
| https://zed.dev/docs/ai/mcp | `context_servers` settings key/shape for MCP servers (command+args+env, or url+headers) | 2026-08-14 |
| https://zed.dev/docs/tasks | Tasks feature: config locations (`~/.config/zed/tasks.json`, `.zed/tasks.json`), task schema incl. `hooks` field, `create_worktree` as the only hook, `$ZED_*` variable interpolation, no agent-panel integration documented | 2026-08-14 |
| https://zed.dev/docs/reference/all-settings | Settings reference grep: no `hook`/`hooks`/`notify` keys; `tasks` and `session` (unrelated "lifecycle" = restore-buffers/worktree-trust) keys only | 2026-08-14 |
| https://zed.dev/docs/configuring-zed | Extension install paths per OS; project settings path `.zed/settings.json`; did not state global settings path or reload/restart behavior (NOT DOCUMENTED in fetched content) | 2026-08-14 |
| https://zed.dev/docs/extensions/developing-extensions | Extension capability categories: languages, debuggers, themes, icon themes, snippets, MCP servers — no agent-lifecycle category | 2026-08-14 |
| https://docs.rs/zed_extension_api/latest/zed_extension_api/trait.Extension.html | Full `Extension` trait method list (language servers, slash commands, context servers, doc indexing, debug adapters) — confirms no agent-lifecycle callback exists in the Rust/WASM API | 2026-08-14 |
| https://raw.githubusercontent.com/zed-industries/zed/main/crates/task/src/task_template.rs | Verbatim `TaskHook` enum (`CreateWorktree` only) and `hooks: HashSet<TaskHook>` field — ground truth for the one real "hooks" feature in Zed | 2026-08-14 |
| https://agentclientprotocol.com/overview/introduction | ACP framed as LSP-for-agents; introductory page did not enumerate methods (see schema page) | 2026-08-14 |
| https://agentclientprotocol.com/protocol/schema (via search) | ACP methods: `session/update` (notifications: message chunks, tool calls, plans), `session/request_permission` (permission requests) — structured RPC, no hook/middleware registration concept | 2026-08-14 |
| https://github.com/zed-industries/zed/issues/57890 | "AI Agent extensibility — Custom Commands, Lifecycle Hooks, and Skills" — CLOSED 2026-05-28, proposed `agent.hooks` map + 5 event names, quoted verbatim | 2026-08-14 |
| https://github.com/zed-industries/zed/discussions/57943 | Companion discussion to #57890, OPEN/unanswered, full proposal text incl. relationship to #52688 and #8325 | 2026-08-14 |
| https://github.com/zed-industries/zed/issues/52688 | "Tool-use hooks/events for the built-in agent" — CLOSED 2026-04-04, filed by git-ai maintainer, confirms "no hooks or events to observe tool use" for first-party agent, proposes `agent/tool_use_start`/`agent/tool_use_end`, mentions ACP JSON-RPC proxy as their current workaround for *external* agents only | 2026-08-14 |
| https://github.com/zed-industries/zed/discussions/34962 | "Hooks 🪝 to execute predefined Agent tasks - From Kiro IDE" — OPEN/unanswered since 2025-07-23, proposes `.zed/hooks.json` with agent-prompt hook bodies (Kiro-style), a competing shape vs. #57890's shell-command style | 2026-08-14 |
| https://github.com/zed-industries/zed/issues/8325 | "Editor hooks that can be configured with tasks" — OPEN since 2025-01-02; general (non-agent) task-trigger-on-event request; 2026 comments explicitly ask for it to extend to "agent hooks similar to Cursor" and link to #57943 | 2026-08-14 |
| WebSearch: "zed.dev changelog hook agent 2026" | August 2026 stable release notes: Agent terminal/web-fetch sandboxing, Git Panel "Skip Hooks" toggle (git hooks, not agent hooks), commit-msg hook fix | 2026-08-14 |
| WebSearch: "zed.dev notify agent panel notification" | Confirms notify toggles are Zed's own OS notification, not user-command; surfaces third-party workarounds (zed-notify extension for Pi/terminal-bell, Pushary push-notification service) that exist *because* Zed has no native hook to build on | 2026-08-14 |
| https://github.com/zed-industries/zed/discussions/54722 | "Desktop notification on agent event (complete/approval needed)" — OPEN/unanswered 2026-04-24, another unmet request in the same space, cites VS Code's OS-notification-on-user-action as precedent | 2026-08-14 |
