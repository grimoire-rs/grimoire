# Codex CLI (openai/codex) — native hook / lifecycle-event mechanism

Research date: **2026-08-14**. Repo state: `main` branch, HEAD around release
`rust-v0.148.0-alpha.15` (published 2026-08-14T03:00:20Z); latest **stable**
tag `rust-v0.147.0` (2026-08-07T01:41:49Z).

## Executive correction to the premise

The brief states grim currently records that Codex **declined** rules/hooks
because it "has no path-scoped instruction mechanism and hooks rejected
upstream." That is **out of date**. Codex has a full native hook system,
shipped and under active, heavy development throughout 2026. The originating
feature request is **openai/codex#2109 "Event Hooks"**
(<https://github.com/openai/codex/issues/2109>), opened **2025-08-09**,
labeled `enhancement`, `hooks`, and **closed as completed 2026-03-27**. As of
this research date there is an open umbrella tracker,
**openai/codex#21753 "Full Claude Code Hook Parity (29+)"**
(<https://github.com/openai/codex/issues/21753>, opened 2026-05-08, still
open, last updated 2026-07-30), explicitly chasing **feature parity with
Claude Code's own hook system**. The internal Rust type is literally named
`ClaudeHooksEngine` (`codex-rs/hooks/src/lib.rs` and used throughout the
crate) — the implementation is a direct, acknowledged port of Claude Code's
hook contract, adapted to Codex's TOML-based config and multi-surface
(CLI/TUI/Desktop/app-server) architecture.

---

## 1. Existence & name

**Exists. Vendor calls it "Hooks" / "Lifecycle hooks."** Official docs:
<https://developers.openai.com/codex/hooks> (redirects 308 to
<https://learn.chatgpt.com/docs/hooks>) and a "Lifecycle hooks" section in
the repo's `docs/config.md`.

- Not marked beta/experimental on the docs page itself. However, the
  **feature-flag gate is literally named `features.hooks`**, a boolean,
  with `features.codex_hooks` documented as **"a deprecated alias"** — i.e.
  the feature shipped under an earlier internal name (`codex_hooks`) before
  being renamed to `hooks`. This is consistent with a feature that
  graduated from an experimental/internal flag to a supported one.
- Origin: openai/codex#2109 (opened 2025-08-09, **closed completed
  2026-03-27**). Umbrella parity tracker openai/codex#21753 (opened
  2026-05-08, open) shows continuous expansion since. A first `oxysoft`
  comment on #21753 indexes pre-existing, narrower hook issues going back to
  low issue numbers (e.g. #8929 "Notify not getting triggered", #11912
  "hook for custom compaction", #14754 "PreToolUse/PostToolUse hook
  events"), meaning hook-shaped asks predate the general "Event Hooks"
  umbrella by a wide margin.
- Very actively developed: in the last two weeks alone (2026-08-01 through
  2026-08-14) there are 10+ merged PRs and a dozen+ open issues touching
  hooks (async hook execution, MCP-tool hook handlers, Windows quoting
  bugs, app-server daemon dispatch, timeout/process-tree cleanup). Treat any
  single detail here as a snapshot of a **moving target**.
- **`notify` is a separate, older, still-present mechanism** — see §6/§11.
  It has been internally re-implemented as a hidden hook
  (`codex-rs/hooks/src/legacy_notify.rs`) that fires on the same "agent turn
  complete" transition, purely for backward compatibility; it is not part
  of the `[hooks]`/`hooks.json` configuration surface.

## 2. Config location(s)

Primary source: `codex-rs/hooks/src/engine/discovery.rs` (handler discovery/
merge) and `codex-rs/config/src/hook_config.rs` (schema types).

**Directory convention + file paths**, both **project** and **global**
scope, in the *same* two shapes:

| Scope | Standalone file | Inline table |
|---|---|---|
| Global/user | `~/.codex/hooks.json` (i.e. `$CODEX_HOME/hooks.json`) | `~/.codex/config.toml` → `[hooks]` |
| Project | `<repo>/.codex/hooks.json` | `<repo>/.codex/config.toml` → `[hooks]` |
| Plugin | `<plugin-root>/hooks/hooks.json` (or manifest-declared path) | — |
| Managed (admin/MDM) | `requirements.toml` → `[hooks]` inline, or a `managed_dir` / `windows_managed_dir` pointing at externally-maintained hook files | — |

`CODEX_HOME` is the standard Codex relocation env var for the whole
`~/.codex` tree (confirmed elsewhere in the repo/docs, not hook-specific);
no hook-specific env var relocates just the hooks file.

**Config layer stack** (`ConfigLayerSource` enum,
`codex-rs/hooks/src/engine/discovery.rs`, corroborated by the generated
`codex-rs/app-server-protocol/schema/typescript/v2/HookSource.ts`):

```ts
export type HookSource = "system" | "user" | "project" | "mdm" | "sessionFlags"
  | "plugin" | "cloudRequirements" | "cloudManagedConfig"
  | "legacyManagedConfigFile" | "legacyManagedConfigMdm" | "unknown";
```

**Merge semantics: sources APPEND, they do not override.** The discovery
function that walks every layer is literally named `append_hook_events` and
is called once per layer/plugin in sequence, pushing into a single
`Vec<ConfiguredHandler>` (`discovery.rs`). The official docs confirm this in
prose: *"Matched hooks from multiple files all run concurrently."* This is
the opposite of ordinary Codex config-key precedence (where a higher layer
normally wins) — hook *entries* are additive across every trusted layer
that has a matcher for the firing event. The `hooks.json`-vs-inline-TOML
question is explicitly reconciled too — a `discovery.rs` comment states:
*"hooks from config TOML and hooks.json converge on the same trust
identity"* (see §9).

**Admin override to *stop* the merge**: `requirements.toml` (only —
`config.toml` is explicitly rejected for this) can set
`allow_managed_hooks_only = true` top-level, which makes Codex **ignore
user, project, and session hook configs** and load only managed hooks from
requirements/managed layers (`docs/config.md`, quoted verbatim):

> "Admins can set top-level `allow_managed_hooks_only = true` in
> `requirements.toml` to ignore user, project, and session hook configs
> while still allowing managed hooks from requirements and managed config
> layers. This setting is only supported in `requirements.toml`; putting it
> in `config.toml` does not enable managed-hooks-only mode."

**Disabling entirely**: `[features] hooks = false` in `config.toml`; an
admin can force it back on via `requirements.toml`'s `[features] hooks =
true` (doc-site paraphrase, medium confidence on the exact override
direction — the `features.hooks` flag itself and the `codex_hooks`
deprecated-alias wording are directly sourced from the live docs page).

## 3. Config schema — verbatim

**This is an array-of-matcher-groups shape, keyed by a named struct with
one field per event (NOT an open string map, though it serializes to one
via serde rename).** Ground truth,
`codex-rs/config/src/hook_config.rs` (quoted in full — this is the entire
config-side type definition, current as of 2026-08-14):

```rust
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HooksFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub hooks: HookEventsToml,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HooksToml {              // shape used for the inline `[hooks]` table in config.toml
    #[serde(flatten)]
    pub events: HookEventsToml,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub state: BTreeMap<String, HookStateToml>,   // per-hook trust/enabled side-table, keyed by synthetic id — see §9
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HookStateToml {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trusted_hash: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HookEventsToml {
    #[serde(rename = "PreToolUse", default)]        pub pre_tool_use: Vec<MatcherGroup>,
    #[serde(rename = "PermissionRequest", default)] pub permission_request: Vec<MatcherGroup>,
    #[serde(rename = "PostToolUse", default)]       pub post_tool_use: Vec<MatcherGroup>,
    #[serde(rename = "PreCompact", default)]        pub pre_compact: Vec<MatcherGroup>,
    #[serde(rename = "PostCompact", default)]       pub post_compact: Vec<MatcherGroup>,
    #[serde(rename = "SessionStart", default)]      pub session_start: Vec<MatcherGroup>,
    #[serde(rename = "SessionEnd", default)]        pub session_end: Vec<MatcherGroup>,
    #[serde(rename = "UserPromptSubmit", default)]  pub user_prompt_submit: Vec<MatcherGroup>,
    #[serde(rename = "SubagentStart", default)]     pub subagent_start: Vec<MatcherGroup>,
    #[serde(rename = "SubagentStop", default)]      pub subagent_stop: Vec<MatcherGroup>,
    #[serde(rename = "Stop", default)]              pub stop: Vec<MatcherGroup>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MatcherGroup {
    #[serde(default)] pub matcher: Option<String>,
    #[serde(default)] pub hooks: Vec<HookHandlerConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type")]
pub enum HookHandlerConfig {
    #[serde(rename = "command")]
    Command {
        command: String,
        #[serde(default, rename = "commandWindows", alias = "command_windows")]
        command_windows: Option<String>,
        #[serde(default, rename = "timeout")]
        timeout_sec: Option<u64>,
        #[serde(default)]
        r#async: bool,
        #[serde(default, rename = "statusMessage")]
        status_message: Option<String>,
        #[serde(default, rename = "additionalContextLimit", skip_serializing_if = "Option::is_none")]
        additional_context_limit: Option<usize>,
    },
    #[serde(rename = "mcp_tool")]
    McpTool {
        server: String,
        tool: String,
        #[serde(default, deserialize_with = "deserialize_mcp_tool_input")]
        input: serde_json::Map<String, serde_json::Value>,
        #[serde(default, rename = "timeout")]
        timeout_sec: Option<u64>,
        #[serde(default, rename = "statusMessage")]
        status_message: Option<String>,
    },
    #[serde(rename = "prompt")]
    Prompt {},
    #[serde(rename = "agent")]
    Agent {},
}
```

So the **wire/authored JSON keys** are: `type` (`"command"` | `"mcp_tool"` |
`"prompt"` | `"agent"`), `command`, `commandWindows` (alias
`command_windows`), `timeout` (seconds — NOT `timeoutSec` in the authored
file; that name only appears in the *generated app-server RPC protocol*
view, a separate serialization boundary), `async`, `statusMessage`,
`additionalContextLimit`, and for `mcp_tool`: `server`, `tool`, `input`.
`prompt` and `agent` are **field-less** — `{"type": "prompt"}` /
`{"type": "agent"}` is the complete entry; what they do at runtime beyond
selecting a built-in behavior is **NOT DOCUMENTED** in any prose I could
find (only the empty-struct schema is confirmed). Only `command` (and,
newly, `mcp_tool`) invoke anything resembling "user-supplied code" in the
brief's sense.

A **real, verbatim, working example** — lifted directly from the project's
own integration test (`codex-rs/exec/tests/suite/hooks.rs`, current on
`main`):

```json
{
  "hooks": {
    "SessionStart": [
      {
        "hooks": [
          { "type": "command", "command": "touch /some/path/session-start-ran" }
        ]
      }
    ]
  }
}
```

TOML equivalent (from `developers.openai.com/codex/hooks`, cross-checked
against the struct above):

```toml
[[hooks.PreToolUse]]
matcher = "^Bash$"

[[hooks.PreToolUse.hooks]]
type = "command"
command = "python3 /enterprise/hooks/policy.py"
command_windows = 'py -3 C:\enterprise\hooks\policy.py'
```

**Matcher syntax**: a regex string matched against the tool/subagent name
(examples from docs and tests: `"Bash"`, `"^apply_patch$"`, `"Edit|Write"`,
`"mcp__filesystem__.*"`); for non-tool events the matcher is drawn from a
fixed small vocabulary instead (see §4 table); `null`/omitted matcher on a
`MatcherGroup` means "match all" (the Rust field is `Option<String>`,
default `None`).

**Stable identity — the brief's key question.** There is **no
author-assigned `id`/`name`/`description` field on an individual hook
entry.** `HooksFile` only carries one optional file-level `description`
(applies to the whole file, not a per-entry label). Individual identity is
entirely **derived and positional**. Confirmed two ways:

1. Plugin declaration key builder
   (`codex-rs/hooks/src/declarations.rs`, test-asserted):
   `format!("{plugin_id}:{source_relative_path}:{event_snake_case}:{group_index}:{handler_index}")`,
   e.g. `"demo@test:hooks/hooks.json:pre_tool_use:0:0"`.
2. The app-server's own listing type embeds the same idea generally, not
   just for plugins — `HookMetadata.ts` (generated,
   `codex-rs/app-server-protocol/schema/typescript/v2/HookMetadata.ts`):
   ```ts
   export type HookMetadata = {
     key: string, eventName: HookEventName, handlerType: HookHandlerType,
     executionMode: HookExecutionMode, matcher: string | null,
     command: string | null, timeoutSec: bigint, statusMessage: string | null,
     additionalContextLimit: number | null, sourcePath: AbsolutePathBuf,
     source: HookSource, pluginId: string | null, displayOrder: bigint,
     enabled: boolean, isManaged: boolean, currentHash: string,
     trustStatus: HookTrustStatus,
   };
   ```
   Every hook has a `key` (the positional composite above) and a
   `currentHash` (content hash of the handler, used for trust — see §9),
   **not** a human-chosen id.

This is corroborated live by an open upstream complaint,
**openai/codex#31469** *"Plugin hooks: support a per-hook name/description
and show it in the hook-trust UI (currently opaque 'Hook 1..N')"*
(<https://github.com/openai/codex/issues/31469>, open, 2026-07-07) and
**openai/codex#25293** *"Hook settings page shows generic Hook N labels
instead of hook names or commands"* (open, 2026-05-30) — i.e. the vendor's
own users are asking for exactly the stable-identity feature a third-party
installer like grim would want, and it does not exist yet. **Practical
consequence for grim**: a composite key of
`(source-file, event, group-index, handler-index)` is deterministic *as
long as grim owns the whole file and never lets a human hand-edit entries
in between grim's own*, but is **not safe for idempotent insert/remove in
the middle of a human- or multi-tool-managed list** — removing an earlier
entry shifts every later index, and (per the trust design in §9) also
invalidates their `trusted_hash` bookkeeping in `HookStateToml`, since that
state map is keyed by the same derived string.

## 4. Event catalogue

**Eleven events, current and complete** as of this research date — sourced
directly from the generated JSON Schema fixtures checked into the repo at
`codex-rs/hooks/schema/generated/*.schema.json` (auto-generated from the
Rust types above; this is the single most authoritative list available,
more current than either the prose docs or the older tracker issue) and
cross-confirmed by `codex-rs/app-server-protocol/schema/typescript/v2/HookEventName.ts`:

```ts
export type HookEventName = "preToolUse" | "permissionRequest" | "postToolUse"
  | "preCompact" | "postCompact" | "sessionStart" | "sessionEnd"
  | "userPromptSubmit" | "subagentStart" | "subagentStop" | "stop";
```

(That's the app-server RPC's camelCase view; the config-file/wire-payload
`const` value for the same event is PascalCase, e.g. `"PreToolUse"` — see
§3/§6.)

| Event (wire `hook_event_name`) | When it fires | Matcher vocabulary | Scope |
|---|---|---|---|
| `SessionStart` | Session begins | `startup`, `resume`, `clear`, `compact` (`source` field) | thread |
| `SessionEnd` | Session ends (main thread only; `reason` field is currently always the literal `"other"` — placeholder for future values) | n/a | thread |
| `UserPromptSubmit` | Before a submitted user prompt is sent to the model | n/a | turn |
| `PreToolUse` | Before a tool call executes | tool name incl. `Bash`, `apply_patch`, `mcp__<server>__<tool>` | turn |
| `PermissionRequest` | Before an approval prompt would otherwise show | tool name | turn |
| `PostToolUse` | After a tool call completes | tool name | turn |
| `PreCompact` / `PostCompact` | Around context compaction | `manual`, `auto` (`trigger` field) | turn |
| `SubagentStart` | A subagent thread launches | subagent/agent type | thread |
| `SubagentStop` | A subagent thread halts | subagent/agent type | thread |
| `Stop` | The (root) turn stops | n/a | turn |

Grouping requested by the brief:
- **Session lifecycle**: `SessionStart`, `SessionEnd`.
- **Prompt submit**: `UserPromptSubmit`.
- **Pre/post tool use**: `PreToolUse`, `PermissionRequest`, `PostToolUse`.
- **File edit / command execution**: not separate events — folded into
  `PreToolUse`/`PostToolUse` matched by tool name (`Bash`, `apply_patch`,
  `Edit`/`Write`-style tools per the matcher examples).
- **Notification**: no first-class `Notification` hook event exists yet in
  the shipped 11 (the parity tracker #21753 lists `Notification` as
  *"Partial — Notify behavior exists, but event semantics need parity
  clarity"* — this refers to the separate legacy `notify` mechanism, §11).
- **Stop/finish**: `Stop`, `SessionEnd`.
- **Compaction**: `PreCompact`, `PostCompact`.
- **Subagent**: `SubagentStart`, `SubagentStop`.
- **Error**: **no dedicated error/failure event ships yet.** The parity
  tracker explicitly lists `PostToolUseFailure`, `StopFailure`,
  `PermissionDenied` as *"Missing"* (as of its last update, 2026-07-30) —
  failures are currently only visible indirectly (e.g. via the normal
  `PostToolUse`/`Stop` payload fields, or as a non-zero exit from Codex's
  own tool execution, not a distinct hook firing).

**Not yet shipped** (per #21753's tracker, so treat as a live roadmap, not
current fact): `Setup`, `UserPromptExpansion`, `PermissionDenied`,
`PostToolUseFailure`, `PostToolBatch`, `TaskCreated`, `TaskCompleted`,
`StopFailure`, `TeammateIdle`, `InstructionsLoaded`, `ConfigChange`,
`CwdChanged`, `FileChanged`, `WorktreeCreate`, `WorktreeRemove`. A distinct
comment thread on #21753 (users `Keesan12`, `SaravananJaichandar`) argues
that name-parity with Claude Code hooks is insufficient without shared
*policy* semantics (`blocking`, `authority`, `terminal_for`,
`resume_capable` fields) — worth reading if grim's portable hook schema
wants to abstract over multiple clients' decision models.

## 5. Invocation

`type: "command"` handlers run as a **child process via a shell**
(`codex-rs/hooks/src/engine/command_runner.rs`):

- **cwd**: the session's working directory (present in every payload as
  `cwd`; process spawned with that as its working directory).
- **Windows**: resolves the shell via the `COMSPEC` env var, defaulting to
  `cmd.exe`, i.e. `cmd /C <command>` (a `commandWindows` field lets the
  author supply a Windows-specific command string instead of relying on
  POSIX-shell quoting working under `cmd.exe`). A currently-open bug,
  **openai/codex#38168** *"Windows: hook commands with embedded quotes
  never execute (cmd /C outer-quote wrap in command_runner.rs) but hooks
  report Completed"* (open, 2026-08-12), shows this path is still fragile.
- **POSIX**: standard shell invocation (`sh -c`-style; exact program not
  independently re-derived from source beyond the Windows `COMSPEC` branch
  — treat as high-confidence by analogy/docs, not a directly quoted line).
- **$PATH**: no hook-specific PATH manipulation found; environment is
  inherited from the Codex process with `scrub_non_inheritable_env_vars`
  applied (`codex-rs/hooks/src/registry.rs`, used by the shared
  `command_from_argv` helper) — i.e. some environment variables are
  deliberately stripped before spawning, but the exact denylist is **NOT
  DOCUMENTED** here (out of scope to chase further; it lives in
  `codex_protocol::shell_environment`).
- **Timeouts**: default **600 seconds**, floor of 1s
  (`timeout_sec.unwrap_or(600).max(1)`,
  `codex-rs/hooks/src/engine/discovery.rs:687`), overridable per-handler via
  the `timeout` config key. `SessionEnd` is a special case with its own
  much shorter bounds: default **1 second**, max **3 seconds**
  (`SESSION_END_DEFAULT_TIMEOUT_SEC = 1`, `SESSION_END_MAX_TIMEOUT_SEC = 3`,
  `codex-rs/hooks/src/events/session_end.rs`) — the code comment explains
  why: *"Keep below app-server's in-process `SHUTDOWN_TIMEOUT`: SessionEnd
  runs during teardown and must leave headroom within the existing
  five-second bound."* A currently-open bug, **openai/codex#27550** *"stdin
  write happens outside the per-hook timeout - a hook that ignores stdin
  can hang the turn forever, and a fast-exiting hook is wrongly marked
  failed"* (open, 2026-06-11), documents a real edge case in this area.
- **Concurrency**: a hard cap of **8 concurrent background (`async: true`)
  hooks** per session, enforced with a `tokio::sync::Semaphore`
  (`MAX_CONCURRENT_ASYNC_HOOKS: usize = 8`,
  `codex-rs/hooks/src/engine/command_runner.rs`); additional async hooks
  queue. **Synchronous (`async: false`, the default)** handlers matched by
  the same event/matcher **run concurrently with each other** (confirmed by
  a doc comment on `max_permission_request_timeout`: *"Matching handlers
  run concurrently, so their aggregate timeout is bounded by this
  maximum"*) and the whole operation blocks on them.
- **Ordering across layers**: each discovered handler is assigned a
  `display_order: i64`/`bigint` as layers are appended in a fixed sequence
  (system → user → project → plugin → managed, per the `HookSource` append
  order in `discovery.rs`); whether this also governs *execution* order
  for sync handlers (vs. being purely a UI/listing order) is **not
  independently confirmed** — treat as medium confidence.
- **`mcp_tool` handlers** invoke an already-configured MCP server's tool
  by name (`server`, `tool`, `input`) rather than spawning a process —
  shipped very recently (**openai/codex#37363 "Recognize MCP tool hook
  configurations"**, merged 2026-08-07).
- **`prompt` / `agent` handlers**: field-less; mechanism of action **NOT
  DOCUMENTED** beyond the empty schema (§3).

## 6. Input payload — verbatim

Delivered as **JSON on stdin** (`command_runner.rs`:
`stdin.write_all(input_json.as_bytes())`, with `.stdin(Stdio::piped())`
when the process is spawned). This is the exact, current, generated
schema — every field, every event — copied verbatim from
`codex-rs/hooks/schema/generated/*.command.input.schema.json` (draft-07
JSON Schema, `additionalProperties: false`, i.e. these are closed/exact
shapes, not "at least these fields"):

**Common to every event**: `cwd` (string), `hook_event_name` (string
const, the event's PascalCase name), `session_id` (string),
`transcript_path` (string | null). Most (all but `SessionEnd`) also carry
`model` (string) and `permission_mode` (enum:
`"default" | "acceptEdits" | "plan" | "dontAsk" | "bypassPermissions"`).

**Turn-scoped events add** `turn_id` (string) — the schema's own
description calls this out explicitly: *"Codex extension: expose the
active turn id to internal turn-scoped hooks."* (i.e. this field is a
Codex addition, not inherited from the Claude Code contract being ported).

Per-event additions:

- **`PreToolUse`**: `tool_name` (string), `tool_use_id` (string),
  `tool_input` (any JSON value — schema literally `true`, i.e.
  unconstrained), plus optional `agent_id`/`agent_type` (not in the
  `required` list — populated for subagent-originated calls).
- **`PermissionRequest`**: `tool_name`, `tool_input` — no `tool_use_id`
  (not required/present here, unlike `PreToolUse`/`PostToolUse`).
- **`PostToolUse`**: everything `PreToolUse` has, plus `tool_response`
  (any JSON value, required).
- **`UserPromptSubmit`**: `prompt` (string, required).
- **`SessionStart`**: `source` (enum `"startup" | "resume" | "clear" |
  "compact"`, required) — no `permission_mode`... wait, it *is* required
  here too. No tool fields (n/a for this event).
- **`SessionEnd`**: minimal — `reason` (string `const: "other"` — only
  literal value currently emitted), no `model`/`permission_mode`/`turn_id`.
- **`PreCompact`/`PostCompact`**: `trigger` (enum `"manual" | "auto"`,
  required), `turn_id`; no `permission_mode`.
- **`Stop`**: `last_assistant_message` (string | null, required),
  `stop_hook_active` (boolean, required — a re-entrancy guard flag, same
  name/purpose as Claude Code's own `stop_hook_active`).
- **`SubagentStart`**: `agent_id` and `agent_type` are **required** here
  (unlike `PreToolUse` where they're optional) — this is the subagent's own
  identity, not the parent's.
- **`SubagentStop`**: adds `agent_transcript_path` (string | null,
  required) and `last_assistant_message`, `stop_hook_active` (same shape as
  `Stop`) alongside `agent_id`/`agent_type`.

**Note the case convention**: input/payload keys are **snake_case**
throughout (`hook_event_name`, `tool_use_id`, `permission_mode`,
`stop_hook_active`, …). This flips to **camelCase** on the way out (§7) —
an intentional asymmetry that exactly mirrors Claude Code's own hook wire
format, further evidence of the deliberate parity port.

**Env vars / argv / template interpolation**: none of the 11 modern hook
events pass data via env vars, argv, or `{{template}}` interpolation into
the command string — it is stdin-JSON only. The **only** place env vars
carry hook-relevant data is plugin-sourced hooks, where the doc site
(secondary source; I could not independently re-derive the exact names
from `command_runner.rs` within this pass — `discovery.rs` does thread a
generic `env: HashMap<String, String>` per hook source into the runtime,
consistent with but not verbatim-proving this) states `PLUGIN_ROOT` and
`PLUGIN_DATA` are set for plugin hook processes. Treat those two exact
names as **medium confidence**.

## 7. Output / response contract — verbatim

**Exit codes** (doc-site, corroborated by the `BlockDecisionWire`/
`PreToolUseDecisionWire` enums below existing specifically to represent a
block decision distinct from a bare non-zero exit):
- `0` = success.
- `2` = a blocking decision, historically-Claude-compatible convention
  (reason expected on stderr for the plain-exit-code path).
- Any other non-zero = failure (treated as `HookRunStatus: "failed"`, not a
  deliberate block).

**stdout handling differs by event class** (doc-site, high confidence
given it matches the schema split below):
- `SessionStart`, `SubagentStart`: plain-text stdout (if not JSON) is
  added as developer/model-visible context.
- `PreToolUse`, `PermissionRequest`, `PostToolUse`: plain-text stdout is
  **ignored** — only a JSON object is meaningful.
- All others: non-JSON stdout is invalid.

**The JSON response schema is generated per-event**
(`*.command.output.schema.json`), all sharing this base shape (fields
present on every event's output schema): `continue` (bool, default
`true`), `stopReason` (string, default null), `suppressOutput` (bool,
default `false`), `systemMessage` (string, default null). Events that can
block additionally carry `decision` and/or `hookSpecificOutput`. Exact,
verbatim, per event:

```jsonc
// pre-tool-use.command.output — full decision surface
{
  "continue": true,                 // default
  "decision": "approve" | "block",  // top-level, coarse decision
  "reason": "string",               // used when decision=block (exit-code-2 compatible path)
  "stopReason": "string",
  "suppressOutput": false,
  "systemMessage": "string",
  "hookSpecificOutput": {
    "hookEventName": "PreToolUse",           // required, const
    "permissionDecision": "allow" | "deny" | "ask",
    "permissionDecisionReason": "string",
    "additionalContext": "string",           // model-visible text
    "updatedInput": <any>                    // rewritten tool_input
  }
}
```

```jsonc
// permission-request.command.output
{
  "continue": true, "stopReason": "string", "suppressOutput": false, "systemMessage": "string",
  "hookSpecificOutput": {
    "hookEventName": "PermissionRequest",
    "decision": {
      "behavior": "allow" | "deny",   // required
      "message": "string",
      "interrupt": false,             // "Reserved for future short-circuiting semantics.
                                       //  PermissionRequest hooks currently fail closed if this field is `true`."
      "updatedInput": <any>,          // "Reserved for a future input-rewrite capability. … fail closed if present."
      "updatedPermissions": <any>     // "Reserved for a future permission-rewrite capability. … fail closed if present."
    }
  }
}
```
(The three "Reserved for future…" fields are quoted verbatim from the
generated schema's `description` — i.e. the vendor ships the wire slots
before the behavior, and **deliberately fails closed** if you populate
them early. Good example of an exact-string trap for a naive integrator.)

```jsonc
// post-tool-use.command.output
{
  "continue": true, "decision": "block", "reason": "string",
  "stopReason": "string", "suppressOutput": false, "systemMessage": "string",
  "hookSpecificOutput": {
    "hookEventName": "PostToolUse",
    "additionalContext": "string",
    "updatedMCPToolOutput": <any>     // rewrite the tool's result before the model sees it
  }
}
```

```jsonc
// stop.command.output / user-prompt-submit.command.output (block-capable, no rich hookSpecificOutput on Stop)
{
  "continue": true, "decision": "block", "reason": "string",
  "stopReason": "string", "suppressOutput": false, "systemMessage": "string"
}
```
Schema comment on `stop.command.output`'s `reason` field, verbatim:
*"Claude requires `reason` when `decision` is `block`; we enforce that
semantic rule during output parsing rather than in the JSON schema."*
— an explicit, named admission that the contract is validated against
**Claude's** semantics, not merely inspired by them.

```jsonc
// session-start.command.output (no decision — can't block a session from starting)
{
  "continue": true, "stopReason": "string", "suppressOutput": false, "systemMessage": "string",
  "hookSpecificOutput": { "hookEventName": "SessionStart", "additionalContext": "string" }
}
```

`PreCompact`/`PostCompact`/`SubagentStart`/`SubagentStop` outputs follow
the same base-shape pattern (`continue`/`stopReason`/`suppressOutput`/
`systemMessage`, ± a narrow `hookSpecificOutput.additionalContext`); no
output schema exists for `SessionEnd` at all (fire-and-forget, matching its
1–3s timeout budget).

**stderr**: goes to the hook's own captured stderr; used as the
human-readable reason on the plain exit-code-2 path. Whether it's shown to
the user, the model, both, or neither is **NOT DOCUMENTED** verbatim in
anything I fetched — the docs only commit to stdout's routing.

**Large output handling ("spill")**: `additionalContext` above a token
threshold is written to disk rather than inlined. Exact constant,
`codex-rs/hooks/src/output_spill.rs`:
```rust
pub(crate) const DEFAULT_HOOK_OUTPUT_TOKEN_LIMIT: usize = 2_500;
```
Configurable per-handler via `additionalContextLimit` (`0` disables
spilling for that handler). Per the field's own doc comment: *"a spilled
preview also includes recovery metadata"* and is written under
`<temp_dir>/hook_outputs/<session_id>/<uuid>.txt` (doc-site, not
independently re-grepped for the exact path template — medium-high
confidence).

## 8. Reliability & limits

- Non-zero exit → `HookRunStatus: "failed"` (enum:
  `"running" | "completed" | "failed" | "blocked" | "stopped"`,
  `codex-rs/app-server-protocol/schema/typescript/v2/HookRunStatus.ts`).
- Malformed JSON on stdout: parsed defensively by
  `codex-rs/hooks/src/engine/output_parser.rs` (604 lines — not exhaustively
  quoted here); a recent merged fix,
  **openai/codex#35194 "Preserve output from hooks that exit before
  reading stdin"** (2026-07-24), shows the parser is actively hardened
  against early-exit/partial-output races.
- Missing binary / spawn failure: legacy `notify` treats this as
  `HookResult::FailedContinue` (does not abort the turn); modern command
  hooks are expected to surface as a `failed` `HookRunStatus` — exact
  turn-level consequence (does a spawn failure ever block?) is **NOT
  DOCUMENTED** verbatim here.
- Timed-out hook process trees are now killed, not merely abandoned —
  **openai/codex#37527 "Terminate timed-out hook process trees"** (merged
  2026-08-08).
- Unbounded stdout/stderr buffering is a known **open** bug:
  **openai/codex#35712 "Hook command runner buffers unbounded stdout/stderr
  in memory"** (open, 2026-07-28).
- Parallelism: see §5 (8 concurrent async hooks; sync matches run
  concurrently with each other; the operation blocks until all synchronous
  matches for that firing resolve).
- Async output delivery: docs state async hooks' output is "delivered at
  next safe point" rather than immediately — consistent with
  **openai/codex#37533 "Support asynchronous command hooks"** (merged
  2026-08-08, very recent) and **openai/codex#34694 "async command hooks
  are skipped, so Claude-format plugin hooks silently lose their background
  handlers"** (open, 2026-07-22) showing this is still rough at the edges.
- A currently-open correctness bug directly relevant to any trampoline
  design: **openai/codex#34477 "Hook-injected `<hook_prompt>` messages
  cause infinite loop, consuming billions of tokens with zero useful
  output"** (open, 2026-07-21) — a cautionary tale for anything that
  injects `additionalContext` unconditionally on every turn.

## 9. Security posture

Vendor's own wording (`developers.openai.com/codex/hooks`, quoted by
paraphrase-fetch, high confidence given the surrounding structural facts
all check out against source):

> "Before a non-managed command hook can run, Codex requires you to review
> and trust the exact hook definition. Codex records trust against the
> hook's current hash, so new or changed hooks are marked for review and
> skipped until trusted."
>
> "Managed hooks from system, MDM, cloud, or requirements.toml sources are
> marked as managed, trusted by policy, and can't be disabled from the user
> hook browser."

This is fully corroborated by source-level types:
- `HookTrustStatus` (generated,
  `codex-rs/app-server-protocol/schema/typescript/v2/HookTrustStatus.ts`):
  ```ts
  export type HookTrustStatus = "managed" | "untrusted" | "trusted" | "modified";
  ```
- Trust is tracked **per derived key**, hashed, and persisted in
  `HookStateToml { enabled: Option<bool>, trusted_hash: Option<String> }`
  inside a `BTreeMap<String, HookStateToml>` keyed by the same synthetic
  identity discussed in §3 (`HooksToml.state`). Editing a trusted hook's
  command changes its hash → `trustStatus` flips to `"modified"` → it is
  skipped again until re-trusted. A `discovery.rs` comment confirms the
  hash is computed so that **the same logical hook defined via TOML or via
  `hooks.json` converges on one trust identity** — i.e. you can't dodge
  review by moving a hook between the two file formats.
- A dedicated CLI surface exists: **`/hooks`** (TUI slash command;
  `codex-rs/tui/src/startup_hooks_review.rs`). Vendor wording: *"Use /hooks
  in the CLI to inspect hook sources, review new or changed hooks, trust
  hooks, or disable individual non-managed hooks. If hooks need review at
  startup, Codex prints a warning that tells you to open /hooks."* A
  literal startup-warning string exists in source and snapshot tests
  (`codex-rs/tui/src/app/tests.rs`,
  `codex-rs/tui/src/snapshots/codex_tui__app__tests__bypass_hook_trust_startup_warning.snap`):
  ```
  ⚠ `--dangerously-bypass-hook-trust` is enabled. Enabled hooks may run without review for this invocation.
  ```
- **Bypass flag, exact and source-confirmed**:
  `codex-rs/utils/cli/src/shared_options.rs`:
  `#[arg(long = "dangerously-bypass-hook-trust", default_value_t = false)]`.
  Its name alone is the vendor's own risk framing. A currently-**open**
  issue, **openai/codex#32491** *"codex exec skips persisted trusted
  project hooks unless --dangerously-bypass-hook-trust is passed"* (open,
  2026-07-11, still open as of 2026-08-09), shows the interaction between
  headless `codex exec` and the trust store is a known rough edge — trust
  earned interactively does **not** reliably carry over to non-interactive
  runs yet.
- Startup-config snapshotting: not independently confirmed as a documented
  "gotcha" the way Claude Code's is; however
  **openai/codex#38339** *"[macOS][Codex App][Plugins/Hooks] Removed plugin
  Stop hook keeps running until full app restart"* (open, 2026-08-13) and
  **openai/codex#30701** *"After deleting all projects, the Hooks page
  shows 'No hooks found', but the plugin-level SessionStart hook still
  takes effect"* (open, 2026-06-30) both strongly imply hook configuration
  **is** snapshotted per-session/app-lifetime in practice, even though I
  found no docs sentence saying so outright. Treat "requires a restart to
  fully drop a removed hook" as **medium-high confidence, evidenced by bug
  reports rather than a direct vendor statement**.
- **openai/codex#35306** *"No trust prompt for project-level hooks causes
  SessionStart hooks to be silently skipped"* (open, 2026-07-25, updated
  2026-08-09) — a real, live gap between intended and actual trust-prompt
  behavior for project-scoped `SessionStart` hooks specifically.

## 10. Third-party installability

**Yes — files, no proprietary tooling required**, with caveats:

- `hooks.json` is plain JSON with `#[serde(deny_unknown_fields)]` at the
  file root (`description`, `hooks` only — no extension point for an
  installer to stash its own metadata inside the file itself without it
  being rejected).
- Inline `[hooks]` in `config.toml` is TOML-splicable the same way grim
  already splices other Codex TOML config (per the brief's framing) — the
  shape is a plain nested table/array-of-tables, nothing exotic.
- **No client restart is strictly required to pick up a new file** in the
  interactive TUI/app-server case — the `/hooks` review flow is explicitly
  designed to catch new/changed hooks and prompt for trust — but a freshly
  *installed and pre-trusted* hook still needs a human (or
  `--dangerously-bypass-hook-trust`) to clear the trust gate before it
  will ever execute; grim cannot silently make a hook "just work" the way
  it can install a passive rule file. This is the single biggest practical
  difference from grim's normal "write the file, done" model.
- `codex exec` (non-interactive/CI usage) **does** run hooks — proved by
  the project's own integration test in
  `codex-rs/exec/tests/suite/hooks.rs` (`exec_hook_trust_bypass_runs_session_start_hook`,
  quoted in §3) — but per §9/#32491 it currently has an inconsistent
  relationship with previously-established trust, and per §5's `--json`
  check below, does not appear to surface hook activity in its structured
  output stream.
- `codex exec --json` streams JSON Lines with documented event types
  `thread.started`, `turn.started`, `turn.completed`, `turn.failed`,
  `item.*`, `error` (`learn.chatgpt.com/docs/non-interactive-mode`,
  paraphrase-fetched) — **no hook-specific event type is documented in that
  list**, despite hooks demonstrably running during `codex exec` (previous
  paragraph). Treat "does hook activity appear in the `--json` stream" as
  **NOT DOCUMENTED / likely no**, a real gap if grim wanted to observe hook
  execution from the outside during a scripted run.
- Managed/enterprise installs go through `requirements.toml` +
  `managed_dir`/`windows_managed_dir`, a distinct admin-only channel that
  bypasses per-hook trust entirely (§9) — not the channel an OCI-distributed
  package manager like grim would use for a normal user-scoped install, but
  relevant if grim ever targets fleet/enterprise deployment.

## 11. Trampoline viability

**Viable for `type: "command"` hooks; NOT viable, or not meaningfully in
scope, for `prompt`/`agent`/`mcp_tool`.**

A single generic command such as `grim hook run --client codex --event <E>`
maps cleanly onto Codex's `command` handler:
- Codex already does exactly this shape natively: spawn an arbitrary
  program, write one JSON object to its stdin, read one JSON object (or
  plain text, event-dependent) from its stdout, apply exit-code semantics.
  There is no JS-module-only or in-process-function-only handler type —
  `command` is a plain OS process from day one.
- The **input schema is per-event but externally identical in kind**
  (JSON on stdin) — a trampoline binary can dispatch on the incoming
  `hook_event_name` field itself rather than needing Codex to select a
  different binary per event.
- The **output schema is per-event** (§7) — a trampoline must know which
  event it's answering for in order to emit the right `hookSpecificOutput`
  shape, but since the input tells it the event, this is a pure
  implementation detail, not a blocker.

**Named blockers**:
1. **No stable per-entry identity** (§3) — grim's installer/updater/remover
   would have to own the *entire* `hooks.json` file (or a dedicated
   `[hooks]` sub-tree it fully controls) rather than surgically
   inserting/updating/removing one named entry, because the only identity
   available is a positional composite key that a human or another tool
   editing the same file could invalidate. This is the sharpest mismatch
   with grim's usual "one managed member, byte-preserving splice" model.
2. **Trust is a hard gate, and it's file/content-hash based, not
   installer-based** — grim cannot pre-authorize its own trampoline entry;
   whatever hash the trampoline command line hashes to must go through the
   same human `/hooks` review (or `--dangerously-bypass-hook-trust`, a
   flag whose name is a deliberate speed bump) as any hand-written hook.
   Every grim update that changes the trampoline's command string
   (e.g. a version-pinned path) re-triggers review.
3. **`codex exec` / CI-friendliness is unproven**: hooks run under `codex
   exec` (confirmed) but interact awkwardly with persisted trust (open bug
   #32491) and are invisible in the `--json` event stream (undocumented) —
   a grim-installed hook meant to run unattended in CI is currently on
   shakier ground than one running interactively where a human can clear
   `/hooks` once.
4. **The feature is a fast-moving target**: 10+ merged hook-related PRs and
   a dozen+ open hook issues in the last two weeks alone (as of
   2026-08-14). Any concrete field name here should be re-verified against
   `codex-rs/hooks/schema/generated/*.schema.json` on `main` before grim
   ships a hard dependency on it — that directory is auto-generated
   straight from the Rust types and is the cheapest single source of truth
   to diff against in future.
5. Blocking semantics vary by event on purpose and by design admit
   "reserved, fails closed" fields (`PermissionRequest.interrupt`,
   `.updatedInput`, `.updatedPermissions`) that already exist in the schema
   but do nothing yet — a trampoline that optimistically populates them
   today gets silently ignored (fail-closed), not an error; worth a code
   comment in grim's own client adapter so a future Codex upgrade that
   activates them doesn't silently change grim's hook's behavior.

Net: a `command`-type trampoline is straightforward to emit; the harder
design problem for grim's portable `hook` artifact kind is less "can we
invoke a shell command" and more "how do we let grim own one addressable
entry inside a file whose native identity model is purely positional,
under a trust system that hashes exactly what we write."

---

## Sources

| URL | What it establishes | Fetched |
|---|---|---|
| https://developers.openai.com/codex/hooks (→ 308 → https://learn.chatgpt.com/docs/hooks) | Official hooks docs: config paths, schema overview, event table, trust model prose, spill behavior | 2026-08-14 |
| https://developers.openai.com/codex/config-reference (→ 308 → https://learn.chatgpt.com/docs/config-file/config-reference) | `notify` key type/description, `features.hooks` + deprecated `codex_hooks` alias | 2026-08-14 |
| https://learn.chatgpt.com/docs/non-interactive-mode | `codex exec --json` documented event types (no hook event type listed) | 2026-08-14 |
| https://github.com/openai/codex (repo root, `docs/config.md`, `docs/exec.md`, `docs/skills.md`, `docs/example-config.md`) | `docs/config.md`'s verbatim "Lifecycle hooks" section (`requirements.toml`, `allow_managed_hooks_only`); confirms docs/ mostly redirects to the hosted docs site now | 2026-08-14 |
| https://github.com/openai/codex/issues/2109 | Origin issue "Event Hooks", opened 2025-08-09, closed completed 2026-03-27 | 2026-08-14 |
| https://github.com/openai/codex/issues/21753 | Umbrella "Full Claude Code Hook Parity (29+)" tracker: shipped/partial/missing event matrix, explicit Claude-parity goal, community debate on policy semantics | 2026-08-14 |
| https://github.com/openai/codex/issues/31469, /25293 | Confirms no per-hook name/description field exists; UI shows generic "Hook N" | 2026-08-14 |
| https://github.com/openai/codex/issues/32491 | `codex exec` + persisted trust interaction gap; names `--dangerously-bypass-hook-trust` | 2026-08-14 |
| https://github.com/openai/codex/issues/35306, /30701, /38339 | Trust-prompt and hook-teardown edge cases (project hooks silently skipped; stale hook survives until restart) | 2026-08-14 |
| https://github.com/openai/codex/issues/27550, /35712, /34477, /34694 | Reliability edge cases: stdin/timeout race, unbounded buffering, hook-injected infinite loop, async hook handlers skipped | 2026-08-14 |
| https://github.com/openai/codex/pull/37644, /37533, /37363, /37538, /37527, /33926, /33895, /35194, /34393, /34416 | Recent (Jul–Aug 2026) merged hook engine changes: generalized handler execution, async command hooks, MCP-tool handlers, execution-mode listing, timeout process-tree kill, Windows quoting fix, SessionEnd hooks, stdin-race fix, context spill limits, TUI warnings | 2026-08-14 |
| https://github.com/openai/codex/blob/main/codex-rs/hooks/src/legacy_notify.rs | Legacy `notify` reimplementation: argv-appended JSON, fire-and-forget, `HookEvent::AfterAgent`, kebab-case historical wire shape, test-locked | 2026-08-14 |
| https://github.com/openai/codex/blob/main/codex-rs/hooks/src/declarations.rs | Plugin hook synthetic key format, `HookHandlerConfig` variant test fixtures | 2026-08-14 |
| https://github.com/openai/codex/blob/main/codex-rs/hooks/src/registry.rs | `Hooks`/`ClaudeHooksEngine` orchestration, `HooksConfig`, `command_from_argv`, env scrubbing | 2026-08-14 |
| https://github.com/openai/codex/blob/main/codex-rs/hooks/src/engine/discovery.rs | Config-layer sources, `append_hook_events` (merge, not override), managed-hooks path resolution, default timeout (600s) | 2026-08-14 |
| https://github.com/openai/codex/blob/main/codex-rs/hooks/src/engine/command_runner.rs | stdin JSON write, `MAX_CONCURRENT_ASYNC_HOOKS = 8`, Windows `COMSPEC` shell resolution | 2026-08-14 |
| https://github.com/openai/codex/blob/main/codex-rs/hooks/src/output_spill.rs | `DEFAULT_HOOK_OUTPUT_TOKEN_LIMIT = 2_500` | 2026-08-14 |
| https://github.com/openai/codex/blob/main/codex-rs/hooks/src/events/session_end.rs | `SESSION_END_DEFAULT_TIMEOUT_SEC = 1`, `SESSION_END_MAX_TIMEOUT_SEC = 3`, and why | 2026-08-14 |
| https://github.com/openai/codex/blob/main/codex-rs/config/src/hook_config.rs | Full, current `HooksFile`/`HooksToml`/`HookEventsToml`/`MatcherGroup`/`HookHandlerConfig` config-side struct definitions with exact serde wire names | 2026-08-14 |
| https://github.com/openai/codex/tree/main/codex-rs/hooks/schema/generated | 20 generated draft-07 JSON Schema files, one input+output pair per event (except SessionEnd, input only) — the authoritative wire contract | 2026-08-14 |
| https://github.com/openai/codex/tree/main/codex-rs/app-server-protocol/schema/typescript/v2 | Generated TS protocol types: `HookEventName`, `HookHandlerType` (`command|mcp_tool|prompt|agent`), `HookSource`, `HookTrustStatus`, `HookScope`, `HookExecutionMode`, `HookRunStatus`, `HookMetadata`, `ConfiguredHookHandler`, `ConfiguredHookMatcherGroup`, `HooksListEntry` | 2026-08-14 |
| https://github.com/openai/codex/blob/main/codex-rs/utils/cli/src/shared_options.rs | Exact `--dangerously-bypass-hook-trust` clap definition | 2026-08-14 |
| https://github.com/openai/codex/blob/main/codex-rs/exec/tests/suite/hooks.rs | Real, working `hooks.json` example; proves `codex exec` executes `SessionStart` hooks | 2026-08-14 |
| https://github.com/openai/codex/releases (rust-v0.147.0 … rust-v0.148.0-alpha.15) | Version/date anchors for "current stable" vs. "HEAD" | 2026-08-14 |
