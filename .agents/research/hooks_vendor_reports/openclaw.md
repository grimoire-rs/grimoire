# OpenClaw — native hook / lifecycle-event mechanism

Research date: 2026-08-14. Client: **openclaw** (multi-channel agent daemon; grim models it
global-only — fixed daemon home `~/.openclaw/`, no per-project workspace in grim's sense).

Repo: `github.com/openclaw/openclaw` (org id 252820863, created 2025-11-24, still pushing
commits same-day as this research: `pushed_at: 2026-08-14T09:08:22Z`). `package.json` version
`2026.8.1` — **calendar versioning**, not semver; there is no "since vX.Y" convention to cite
for feature-gating. Homepage `https://openclaw.ai`. License field on GitHub: `"Other"`
(`NOASSERTION` SPDX — a `LICENSE`/`SECURITY.md` exists in-repo but I did not read the full
license text; flag before treating any redistribution assumption as settled).

**Access note:** `docs.openclaw.ai/*` returned **HTTP 403** to WebFetch on every attempt
(likely bot-fight/Cloudflare — this is a fetch-tool limitation, not evidence the pages don't
exist). All findings below are sourced instead from the docs **source markdown in the GitHub
repo** (`docs/**/*.md`, `main` branch), fetched verbatim via `gh api .../contents/<path>` +
base64 decode — i.e. the same content the doc site renders, read as raw text rather than
through a summarizing WebFetch pass, so exact strings below are copy-pasted, not paraphrased.
Two early exploratory `WebSearch`/`WebFetch` calls surfaced several **look-alike mirror
domains** (`openclawlab.com`, `openclawcn.com`, `openclaw-ai.com`, `open-claw.bot`,
`team400.ai`, `lumadock.com`, `stack-junkie.com`, a `*.cncfstack.com` host) serving what looks
like scraped/cloned OpenClaw docs content under different branding. None of those were used as
sources here — everything below traces to the GitHub org `openclaw/openclaw` (386k stars, 81k
forks per the API response — a very large, presumably legitimate project) or to
`raw.githubusercontent.com` pulls of its own `docs/` tree. **[unofficial]** if any of those
mirror domains are cited elsewhere, treat them as unverified leads only.

---

## 1. Existence & name

OpenClaw has **three distinct extension surfaces** that all get called "hooks" in some
context — this is the single most important disambiguation for this report:

| Surface | What it is | Doc |
|---|---|---|
| **Internal hooks** | File-based (`HOOK.md` + `handler.ts/js`) scripts that run inside the Gateway process on coarse command/lifecycle events (`/new`, `/reset`, `/stop`, session compaction, gateway startup/shutdown, message flow). Operator-installed, side-effect-oriented. | `/automation/hooks` |
| **Typed plugin hooks** | In-process TypeScript lifecycle hooks (`api.on("before_tool_call", handler, opts)`) registered from inside an OpenClaw **plugin**. Ordered, can block/rewrite/require-approval. This is the closest thing to a "policy hook" system. | `/plugins/hooks` |
| **Webhooks** (inbound) | HTTP endpoints (`POST /hooks/wake`, `/hooks/agent`, `/hooks/<name>`) that let **external systems** trigger an OpenClaw agent turn. This is the *reverse* direction from what this research brief means by "hook" — it's OpenClaw receiving a callback, not OpenClaw emitting one. | `/automation/cron-jobs#webhooks` |

Quoting the disambiguation table verbatim from `docs/automation/hooks.md`:

> | If you want to... | Use... | Why |
> | Save a snapshot on `/new`, log `/reset`, call an external API after `message:sent`, or add coarse operator automation | Internal hooks (`HOOK.md`, this page) | File-based hooks are meant for operator-managed side effects and command/lifecycle automation |
> | Rewrite prompts, block tools, cancel outbound messages, or add ordered middleware/policy | Typed plugin hooks via `api.on(...)` | Typed hooks have explicit contracts, priorities, merge rules, and block/cancel semantics |
> | Add telemetry-only export or observability | Diagnostic events | Observability is a separate event bus, not a policy hook surface |

**Stability:** No `beta`/`experimental`/feature-flag marker found anywhere in either hooks doc
or the plugin-hooks doc. Both surfaces read as fully supported, current, documented features.
No explicit "introduced in version X" statement was found either (consistent with the
calendar-versioning scheme — this project doesn't appear to version-gate docs that way).
Some *sub-parts* are explicitly marked deprecated (see §14 below) but the hook mechanism
itself is not.

`openclaw hooks list` distinguishes standalone internal hooks from plugin-bundled ones, which
show as `plugin:<id>` and cannot be enabled/disabled independently (`docs/cli/hooks.md`):

> Plugin-managed hooks show `plugin:<id>` in `hooks list` and cannot be enabled/disabled here;
> enable or disable the owning plugin instead.

---

## 2. Config location(s)

**Main config file** — single file, no separate project/global split in grim's sense (there is
no per-project config file at all; OpenClaw's only per-project-like concept is a per-*agent*
`workspace` directory, set inside the one daemon config — see §10 disambiguation):

- Default path: **`~/.openclaw/openclaw.json`** (`docs/gateway/configuration.md`: *"OpenClaw
  reads an optional JSON5 config from `~/.openclaw/openclaw.json`. If the file is missing,
  OpenClaw uses safe defaults."*)
- Format: **JSON5** (comments + trailing commas allowed), not plain JSON, not YAML/TOML.
  Examples in the docs are fenced ` ```json5 `.
- Override: **`OPENCLAW_CONFIG_PATH`** env var — *"Override the config file path (default
  `~/.openclaw/openclaw.json`)."* (`docs/help/environment.md`)
- Splitting: supports `$include` directives (e.g. `plugins: { $include: "./plugins.json5" }`),
  confined to the config directory unless `OPENCLAW_INCLUDE_ROOTS` widens it.
- The active config path **must be a regular file** — symlinked `openclaw.json` gets its
  *target* replaced by OpenClaw's atomic (rename-based) writes, not written through.

**`$OPENCLAW_HOME` — corrected finding.** The brief flagged this as "referenced but never
defined on any page fetched." I found the definition. It lives at `docs/help/environment.md`
(rendered path presumably `/help/environment`) — a page that, like all `docs.openclaw.ai`
pages, 403'd for WebFetch in this session, which is almost certainly why it was missed
previously if the earlier pass relied on the doc site rather than the GitHub source. Verbatim:

> `OPENCLAW_HOME` | Override the home directory used for internal OpenClaw path defaults
> (`~/.openclaw/`, agent dirs, sessions, credentials, installer onboarding, and the default dev
> checkout). Useful when running OpenClaw as a dedicated service user.
>
> ### `OPENCLAW_HOME`
> When set, `OPENCLAW_HOME` replaces the system home directory (`$HOME` / `os.homedir()`) for
> internal OpenClaw path defaults. This includes the default state directory, config path,
> agent directories, credentials, installer onboarding workspace, and the default dev checkout
> used by `openclaw update --channel dev`.
>
> `OPENCLAW_HOME` does not grant ownership of the OS account's native Gateway service. Gateway
> service-management commands treat a relocated home as isolated state; use the OS account home
> and a named profile when a separate native service identity is required.
>
> **Precedence:** `OPENCLAW_HOME` > `$HOME` > `USERPROFILE` > Termux `PREFIX` home fallback on
> Android > `os.homedir()`
>
> `OPENCLAW_HOME` can also be set to a tilde path (e.g. `~/svc`), which gets expanded using the
> same OS home fallback chain before use.
>
> Explicit path variables such as `OPENCLAW_STATE_DIR`, `OPENCLAW_CONFIG_PATH`, and
> `OPENCLAW_GIT_DIR` still take precedence.

So: **`OPENCLAW_HOME` is a `$HOME`-replacement, not a `~/.openclaw`-replacement** — shape
matches grim's own `GEMINI_CLI_HOME` semantics (home-dir swap, `.openclaw` segment still
appended), *not* `CLAUDE_CONFIG_DIR`'s shape (which replaces the whole `~/.claude` outright).
Confirmed independently in source at `src/config/paths.ts` (`resolveRequiredHomeDir`, and a
`OPENCLAW_HOME` isolation check for native-service naming at line ~163). Related override vars
from the same table: `OPENCLAW_STATE_DIR` (default `~/.openclaw`), `OPENCLAW_WORKSPACE_DIR`,
`OPENCLAW_PROFILE`, `OPENCLAW_GIT_DIR`.

**Hook directory discovery** — four sources, in this precedence order (`docs/automation/hooks.md`,
"Hook discovery"):

1. **Bundled hooks** — shipped inside OpenClaw itself.
2. **Plugin hooks** — bundled inside installed plugins; *"can override bundled hooks with the
   same name."*
3. **Managed hooks** — `~/.openclaw/hooks/` — *"(user-installed, shared across workspaces); can
   override bundled and plugin hooks."* Extra directories from `hooks.internal.load.extraDirs`
   share this precedence tier. **This is the directory grim would install into for global
   scope.**
4. **Workspace hooks** — `<workspace>/hooks/` (per-agent) — *"disabled by default until
   explicitly enabled."* *"Workspace hooks can add new hook names but cannot override bundled,
   managed, or plugin-provided hooks with the same name."*

Multiple sources do **not** silently merge on conflict — precedence is a strict override
ladder (bundled < plugin < managed < workspace *for new names only*), and per-name collision
resolution is explicit and documented, not "last writer wins" by load order.

The Gateway does **not** scan these directories at all until hooks are opted into:

> The Gateway skips internal hook discovery on startup until internal hooks are configured.
> Enable a bundled or managed hook with `openclaw hooks enable <name>`, install a hook pack, or
> set `hooks.internal.enabled=true` to opt in.

**Hook packs** (installable bundles): npm packages that export hooks via `openclaw.hooks` in
their `package.json`, installed with `openclaw plugins install <path-or-spec>` (registry-only
npm specs; git/URL/file specs and semver ranges rejected for npm installs). Landed under
`~/.openclaw/hooks/<id>` per `docs/cli/hooks.md`: *"Install copies the pack into
`~/.openclaw/hooks/<id>`, enables its hooks under `hooks.internal.entries.*`, and records
install provenance in shared SQLite state."*

---

## 3. Config schema — verbatim

**It is a named map, not an array.** Every hook-config example in the docs uses an
object keyed by hook name under `hooks.internal.entries`:

```json
{
  "hooks": {
    "internal": {
      "enabled": true,
      "entries": {
        "session-memory": { "enabled": true },
        "command-logger": { "enabled": false }
      }
    }
  }
}
```

Per-hook custom env values (read by the handler, also satisfy `requires.env` eligibility):

```json
{
  "hooks": {
    "internal": {
      "entries": {
        "my-hook": {
          "enabled": true,
          "env": { "MY_CUSTOM_VAR": "value" }
        }
      }
    }
  }
}
```

Extra hook directories:

```json
{
  "hooks": {
    "internal": {
      "load": { "extraDirs": ["/path/to/more/hooks"] }
    }
  }
}
```

CLI mutation is idempotent and targeted, not a raw-file edit:

- `openclaw hooks enable <name>` → sets `hooks.internal.entries.<name>.enabled = true`
  **and** flips the master `hooks.internal.enabled` switch on. *"Named entries remain an
  allowlist even when the master flag is true."* Fails if the hook doesn't exist, is
  plugin-managed, or isn't eligible.
- `openclaw hooks disable <name>` → sets `hooks.internal.entries.<name>.enabled = false`.

**Stable identity for a third-party installer:** the hook's directory/name **is** the config
key (`hooks.internal.entries.<name>`), so a name is exactly one JSON object a tool can
add/flip/remove idempotently. `HOOK.md` frontmatter additionally supports `hookKey` — *"Config
key override (defaults to the hook name)"* — letting an installer decouple the on-disk
directory name from the config key if needed. This is a genuinely clean shape for grim: own a
directory under `~/.openclaw/hooks/grim-<slug>/` and a matching `hooks.internal.entries.grim-<slug>`
config object; both add and remove are single, well-scoped writes.

**Retired legacy shape** (do not emulate): `hooks.internal.handlers` — an older way to register
a hook module directly in config without a `HOOK.md` directory — was removed:

> `hooks.internal.handlers` is retired and is no longer loaded or accepted by normal config
> validation. Before running `openclaw doctor --fix`, move each registered module into a
> managed or workspace hook directory with `HOOK.md` and a handler file. Doctor removes the
> retired registrations; it does not create executable hook files.

**HOOK.md frontmatter schema** (verbatim example):

```markdown
---
name: my-hook
description: "Short description of what this hook does"
metadata:
  { "openclaw": { "emoji": "🔗", "events": ["command:new"], "requires": { "bins": ["node"] } } }
---

# My Hook

Detailed documentation goes here.
```

`metadata.openclaw` fields:

| Field | Description |
|---|---|
| `emoji` | Display emoji for CLI |
| `events` | Array of events to listen for |
| `export` | Named export to use (defaults to `"default"`) |
| `os` | Required platforms (e.g., `["darwin", "linux"]`) |
| `requires` | Required `bins`, `anyBins`, `env`, or `config` paths |
| `always` | Bypass eligibility checks (boolean) |
| `hookKey` | Config key override (defaults to the hook name) |
| `homepage` | Docs URL shown by `openclaw hooks info` |
| `install` | Installation methods |

**Matcher/filter syntax — two different vocabularies depending on surface:**

- Internal hooks match by **exact event key string** (`"command:new"`) or a **bare family
  name** (`"command"`, `"session"`, `"agent"`, `"gateway"`, `"message"`) to catch every action
  in that family. No globs, no regex. *"OpenClaw core emits nothing else, so any other name is
  almost always a typo that leaves the hook silently dead ... The hook loader logs a warning
  for such names (for example `command:nwe`), and `openclaw hooks info <name>` flags them."*
- Typed plugin hooks (`before_tool_call`/`after_tool_call`) match by an **exact, non-empty list
  of canonical OpenClaw tool ids** (`exec`, `apply_patch`, `spawn_agent`, ...) via the
  `matcher` option. *"Omit to match all tools. Empty lists, wildcards, blanks, and
  provider-specific aliases are invalid."* — i.e. globs are explicitly rejected, not just
  unsupported.

**A separate, unrelated `hooks.*` schema exists for inbound webhooks** (see §disambiguation in
§1 and full detail in §12) — `hooks.enabled`, `hooks.token`, `hooks.path`,
`hooks.mappings` (an **array**, unlike `hooks.internal.entries`), `hooks.allowedAgentIds`,
`hooks.allowRequestSessionKey`, `hooks.allowedSessionKeyPrefixes`, `hooks.defaultSessionKey`,
`hooks.gmail.*`. Anyone designing a portable schema keyed on the literal string `"hooks"`
should know OpenClaw itself already overloads that root key for two unrelated features
(lifecycle hooks vs. inbound HTTP triggers) plus a third, `plugins.entries.<id>.hooks.timeoutMs`
/ `.timeouts.<hookName>`, which is *timeout config for typed plugin hooks*, not hook
registration itself (typed hooks are registered only in code via `api.on`, never in JSON).

---

## 4. Event catalogue

### Internal hooks (`docs/automation/hooks.md`) — verbatim table

| Event | When it fires |
|---|---|
| `command:new` | `/new` command issued |
| `command:reset` | `/reset` command issued |
| `command:stop` | `/stop` command issued |
| `command` | Any command event (general listener) |
| `session:auto-reset` | A daily or idle reset replaces the current session |
| `session:compact:before` | Before compaction summarizes history |
| `session:compact:after` | After compaction completes |
| `session:patch` | When session properties are modified |
| `agent:bootstrap` | Before workspace bootstrap files are injected |
| `gateway:startup` | After channels start and hooks are loaded |
| `gateway:shutdown` | When gateway shutdown begins |
| `gateway:pre-restart` | Before an expected gateway restart |
| `message:received` | Inbound message from any channel |
| `message:transcribed` | After audio transcription completes |
| `message:preprocessed` | After media and link preprocessing completes or is skipped |
| `message:sent` | Outbound send attempted (`context.success` has the result) |

Bundled hooks shipping on these events, and what they do:

| Hook | Events | What it does |
|---|---|---|
| `session-memory` | `command:new`, `command:reset`, `session:auto-reset` | Saves session context to `<workspace>/memory/` |
| `bootstrap-extra-files` | `agent:bootstrap` | Injects additional bootstrap files from glob patterns |
| `command-logger` | `command` | Logs all commands to `~/.openclaw/logs/commands.log` |
| `compaction-notifier` | `session:compact:before`, `session:compact:after` | Sends visible chat notices when session compaction starts/ends |
| `boot-md` | `gateway:startup` | Runs `BOOT.md` when the gateway starts |

Group by the brief's taxonomy:
- **Session lifecycle:** `session:auto-reset`, `session:compact:before/after`, `session:patch`
- **Prompt submit:** none exactly — closest is `command:new`/`command:reset` (session-level, not per-prompt) and the *typed* `before_prompt_build`/`before_agent_run` (see below)
- **Pre/post tool use:** none in internal hooks — this is typed-plugin-hook-only (`before_tool_call`/`after_tool_call`)
- **File edit:** none directly named; `derivedPaths` on `before_tool_call` (typed) is the closest
- **Command execution:** `command:new/reset/stop`, bare `command`
- **Notification:** `compaction-notifier`'s chat notices are the closest built-in example; no dedicated "notify" event name
- **Stop/finish:** `command:stop`, `gateway:shutdown`, `gateway:pre-restart`
- **Compaction:** `session:compact:before/after`
- **Subagent:** none in internal hooks (typed-only: `subagent_spawned`/`subagent_ended`)
- **Error:** **NOT DOCUMENTED** — no internal-hook event fires specifically on agent/tool error

### Typed plugin hooks (`docs/plugins/hooks.md`) — full catalog, **bold** = accepts a decision (block/cancel/override/require-approval); rest are observation-only

**Agent turn:** `before_model_resolve`, `agent_turn_prepare`, `before_prompt_build`,
**`before_agent_run`**, **`before_agent_reply`**, **`before_agent_finalize`**, `agent_end`,
`heartbeat_prompt_contribution`

**Conversation observation:** `model_call_started`/`model_call_ended` (sanitized metadata only —
*"No prompt or response content"*), `llm_input`, `llm_output`

**Tools:** **`before_tool_call`**, `after_tool_call`, `resolve_exec_env`,
**`tool_result_persist`**, **`before_message_write`**

**Messages/delivery:** **`inbound_claim`**, `channel_pairing_requested`, `message_received`,
**`message_sending`**, **`reply_payload_sending`**, `message_sent`, **`before_dispatch`**,
**`reply_dispatch`**

**Sessions/compaction:** `session_start`/`session_end` (`reason` ∈ `new`, `reset`, `idle`,
`daily`, `compaction`, `deleted`, `shutdown`, `restart`, `unknown`), `before_compaction`/
`after_compaction`, `before_reset`

**Subagents:** `subagent_spawned`/`subagent_ended`, `subagent_delivery_target`,
`subagent_spawning` (deprecated)

**Lifecycle:** `gateway_start`/`gateway_stop`, `cron_reconciled`, `cron_changed`,
**`before_install`**, **`skill_proposal_evaluate`**, `skill_proposal_changed`, `skill_changed`

No dedicated generic "error" event either, though `after_tool_call` observes *"tool results,
errors, and duration"* and `subagent_ended.outcome` can be `"error"`.

**Legacy bridge, explicitly non-overlapping:** the old Plugin SDK `api.registerHook` only
reaches the *internal* event system (`command:new`, `gateway:startup`, `message:received`,
...); registering a typed name like `before_tool_call` through it produces a warning, not a
silent no-op, pointing at `api.on(...)` instead.

---

## 5. Invocation

**This is not a subprocess model at all.** Both hook kinds are **JavaScript/TypeScript
modules that the Gateway's own Node.js process imports and calls in-process** — there is no
shell, no argv, no separate child process, no working-directory concept in the sense a
shell-hook system has one.

- **Internal hooks:** a directory `my-hook/{HOOK.md, handler.ts}`. *"The handler file can be
  `handler.ts`, `handler.js`, `index.ts`, or `index.js`."* The named export (default `"default"`,
  overridable via `metadata.export`) is a function `async (event) => {...}` called directly by
  the Gateway's hook runner.
- **Typed plugin hooks:** a plugin entry file exporting `definePluginEntry({ id, name,
  register(api) { api.on("before_tool_call", handler, opts) } })`. `handler` is a plain
  in-process async function `(event, ctx) => result`.

There is no documented shell, `$PATH` search, or working-directory setting for the hook
*harness* itself — if a handler wants to run an external command, it does so itself with
Node's own `child_process` APIs (the docs' own example does exactly this, see §6).

**Ordering / concurrency:**
- Internal hooks: **NOT DOCUMENTED**. No `priority` field, no stated parallel-vs-sequential
  guarantee anywhere in `docs/automation/hooks.md` for ordinary events. (Only the two gateway
  shutdown events have a stated wait *budget* — see §8 — which implies waiting happens, but
  says nothing about ordering among multiple hooks on the same event.)
- Typed plugin hooks: explicit and documented. *"Handlers that can return decisions or
  modifications run sequentially in descending `priority`; same-priority handlers keep
  registration order. Observation-only handlers run in parallel, and fire-and-forget
  observation dispatches can overlap with later events. Do not use priority to order
  observation side effects."*

**Timeouts:** see §8 — handled per-hook-kind with specific default budgets, not a single global
value, and not configurable in the *internal*-hooks config surface at all (only typed plugin
hooks expose operator-settable `timeoutMs`/`timeouts.<hookName>`).

---

## 6. Input payload — verbatim

**Not JSON on stdin, not env vars, not argv, not string templating.** The event is a **native
JS object** passed as a function argument. There is no serialization boundary at all in the
common case (unless the handler's own code chooses to shell out and serialize something
itself).

**Internal hook event shape**, per `docs/automation/hooks.md`:

> Each event includes: `type`, `action`, `sessionKey`, `timestamp`, `messages`, and `context`
> (event-specific data).

Per-event `context` field lists (all verbatim from the same page):

- `command:new`/`command:reset`: `context.sessionEntry`, `context.previousSessionEntry`,
  `context.commandSource`, `context.senderId`, `context.workspaceDir`, `context.cfg`
- `command:stop`: `context.sessionEntry`, `context.sessionId`, `context.commandSource`,
  `context.senderId`
- `session:auto-reset`: `context.sessionEntry`, `context.reason` (`daily`/`idle`),
  `context.transcriptArchived`, `context.nextSessionId`, `context.nextSessionKey`,
  `context.agentId`, `context.workspaceDir`, `context.storePath`, `context.cfg`
- `message:received`: `context.from`, `context.content`, `context.channelId`, `context.media`,
  `context.originalMedia`, `context.mediaStagingPending`, `context.metadata`
  (`senderId`/`senderName`/`guildId`, provider-specific)
- `message:sent`: `context.to`, `context.content`, `context.success`, `context.channelId`,
  plus `context.error` on failure
- `message:transcribed`: `context.transcript`, `context.from`, `context.channelId`,
  `context.media`
- `message:preprocessed`: `context.bodyForAgent`, `context.from`, `context.channelId`
- `agent:bootstrap`: `context.bootstrapFiles` (**mutable array**), `context.agentId`
- `session:patch`: `context.sessionEntry`, `context.patch`, `context.cfg` — *"the context is a
  clone, so handlers cannot mutate the live session entry"*
- `session:compact:before`: `messageCount`, `tokenCount`; `session:compact:after` adds
  `compactedCount`, `summaryLength`, `tokensBefore`, `tokensAfter`
- `gateway:shutdown`: `reason`, `restartExpectedMs`; `gateway:pre-restart`: same shape, only
  fires when a finite `restartExpectedMs` is supplied

Real handler example, verbatim:

```typescript
const handler = async (event) => {
  if (event.type !== "command" || event.action !== "new") {
    return;
  }
  console.log(`[my-hook] New command triggered`);
  event.messages.push("Hook executed!");
};
export default handler;
```

And the docs' own example of a hook shelling out to an external command (proves handlers can
freely use `child_process`, directly relevant to §11 trampoline viability):

```typescript
import { execFile } from "node:child_process";
import { promisify } from "node:util";
const execFileAsync = promisify(execFile);

export default async function handler(event) {
  if (event.type !== "gateway" || event.action !== "pre-restart") {
    return;
  }
  const restartInSeconds = Math.ceil(event.context.restartExpectedMs / 1000);
  await execFileAsync("openclaw", [
    "system", "event", "--mode", "now", "--text",
    `Gateway restarting in ~${restartInSeconds}s (${event.context.reason}). Checkpoint now.`,
  ]);
}
```

**Typed plugin hook event shape** — varies per hook name (it's a typed contract per event, not
one universal envelope). For `before_tool_call`, verbatim field list:

> - `event.toolName`
> - `event.params`
> - optional `event.toolKind` and `event.toolInputKind` ... e.g. outer code-mode `exec` calls
>   use `toolKind: "code_mode_exec"` and `toolInputKind: "javascript" | "typescript"`
> - optional `event.derivedPaths`, best-effort host-derived target path hints (e.g. for
>   `apply_patch`) — *"these paths may be incomplete or over-approximate"*
> - optional `event.runId`, `event.toolCallId`
> - `ctx.agentId`, `ctx.sessionKey`, `ctx.sessionId`, `ctx.runId`, `ctx.toolKind`,
>   `ctx.toolInputKind`, diagnostic `ctx.trace`
> - optional `ctx.abortSignal` (aborts if the tool call is cancelled)
> - optional `ctx.requester` — `channel`, `accountId`, `senderId`, `senderIsOwner`,
>   provider-native `roleIds`. *"Missing fields are unproven, not false assurances; fail closed
>   when policy requires them."*

Both agent-and-tool typed hook contexts can also carry `trace`, described as *"a read-only
W3C-compatible diagnostic trace context that plugins may pass into structured logs for OTEL
correlation."*

Every hook also receives `event.context.pluginConfig` — *"the resolved config for the plugin
that registered that handler ... without mutating the shared event object other plugins see."*

---

## 7. Output / response contract — verbatim

Two entirely different contracts depending on hook kind; **neither uses an exit code**,
because neither is a subprocess.

### Internal hooks
Side-channel mutation only, no return-value contract for policy decisions (internal hooks
*cannot* block anything — see the internal-vs-typed split in §1). The one output mechanism is
pushing strings onto the mutable `event.messages` array, and even that is honored only for a
subset of events:

> Strings pushed to `event.messages` are delivered back to the chat only for `command:new` and
> `command:reset` (routed as a reply to the originating conversation) and for
> `session:compact:before` / `session:compact:after` (sent as compaction status notices). All
> other events, including `command:stop`, `message:*`, `agent:bootstrap`, `session:patch`, and
> `gateway:*`, ignore pushed messages.

Thrown exceptions: the docs give behavioral advice (*"Handle errors gracefully. Wrap risky
operations in try/catch; do not throw so other handlers can run."*) but **NOT DOCUMENTED**:
the exact runner-level catch/log behavior when a handler *does* throw (i.e., whether it's
logged, to where, and whether it truly does not affect sibling handlers) is not spelled out
beyond that best-practice line — treat "other handlers still run" as strongly implied, not
proven with an exact log-format citation.

stdout/stderr: **NOT DOCUMENTED** as a concept at all for internal hooks — there's no
stdout/stderr in an in-process function call; anything a handler `console.log`s presumably
lands in the Gateway's own log stream (`openclaw logs`), consistent with the troubleshooting
tip *"Check gateway logs: `openclaw logs --follow | grep -i hook`"*, but no explicit statement
ties `console.log` output to a specific log destination/format.

### Typed plugin hooks
A real typed return object, per hook kind. Fullest-documented example, `before_tool_call`,
verbatim:

```typescript
type BeforeToolCallResult = {
  params?: Record<string, unknown>;
  block?: boolean;
  blockReason?: string;
  requireApproval?: {
    title: string;
    description: string;
    severity?: "info" | "warning" | "critical";
    timeoutMs?: number;
    /** @deprecated Unresolved approvals always deny. */
    timeoutBehavior?: "allow" | "deny";
    allowedDecisions?: Array<"allow-once" | "allow-always" | "deny">;
    pluginId?: string;
    onResolution?: (
      decision: "allow-once" | "allow-always" | "deny" | "timeout" | "cancelled",
    ) => Promise<void> | void;
  };
};
```

Guard semantics, verbatim:

> - `block: true` is terminal and skips lower-priority handlers.
> - `block: false` is treated as no decision.
> - `params` rewrites the tool parameters for execution.
> - `requireApproval` pauses the agent run and asks the user through plugin approvals. `/approve`
>   can approve both exec and plugin approvals.
> - A lower-priority `block: true` can still block after a higher-priority hook requested
>   approval.
> - `onResolution` receives the resolved decision: `allow-once`, `allow-always`, `deny`,
>   `timeout`, or `cancelled`.

**So yes — a typed plugin hook (`before_tool_call`) can definitely block a tool call**, rewrite
its params, or force a human-in-the-loop approval gate. Internal hooks cannot do any of this.

Other typed hooks return their own shapes (not exhaustively quoted here for space —
`resolve_exec_env` returns `Record<string,string>`; `skill_proposal_evaluate` returns
`{ evaluatorVersion, mode, decision: "pass"|"revise"|"block", summary, metrics, findings }`;
`message_sending`/`reply_payload_sending` can rewrite or cancel outbound content). The
`skill_proposal_evaluate` example, verbatim:

```typescript
api.on(
  "skill_proposal_evaluate",
  async (event) => {
    const score = await evaluateBundle(event.candidate, event.baseline);
    return {
      evaluatorVersion: "rules-2026-07",
      mode: "baseline-comparison",
      decision: score.regressed ? "revise" : "pass",
      summary: score.summary,
      metrics: score.metrics,
      findings: score.findings,
    };
  },
  { registrationId: "quality-regression", timeoutMs: 90_000 },
);
```

---

## 8. Reliability & limits

**Timeouts are per-hook-kind, not global, and internal hooks have no general default at all:**

| Hook kind | Default budget | Override |
|---|---|---|
| `gateway:shutdown` (internal) | 5s (*"best-effort and bounded so shutdown continues if a handler stalls"*) | not documented as configurable |
| `gateway:pre-restart` (internal) | 10s | not documented as configurable |
| ordinary internal hook events (`command:*`, `message:*`, etc.) | **NOT DOCUMENTED** | n/a |
| `before_tool_call` / `before_install` (typed, policy hooks) | 15s | `hooks.timeoutMs` / `hooks.timeouts.<hookName>` per plugin, up to 600000ms |
| `gateway_stop` (typed) | 5s | same as above |
| `message_sending` / `reply_payload_sending` (typed) | 15s | same as above |
| `session_end` shutdown/restart drain (typed) | **2 seconds total across all active sessions and handlers**, not per-handler | not documented as configurable |
| any other typed hook | runner default (value not stated) unless `api.on(..., { timeoutMs })` set at registration | `hooks.timeoutMs`/`hooks.timeouts.<hookName>` overrides the plugin-authored value |

Operator override example, verbatim:

```json
{
  "plugins": {
    "entries": {
      "my-plugin": {
        "hooks": {
          "timeoutMs": 30000,
          "timeouts": { "before_prompt_build": 90000, "agent_end": 60000 }
        }
      }
    }
  }
}
```

**Fail-open vs fail-closed on timeout** — explicitly documented and hook-kind-specific:

> Policy hooks `before_tool_call` and `before_install` use a 15-second default per handler. A
> timeout fails closed: the tool call or installation is rejected instead of continuing without
> a policy decision.

vs.

> Outbound modifying hooks `message_sending` and `reply_payload_sending` use a 15-second default
> per handler. If one times out, OpenClaw logs the plugin error and continues with the latest
> payload...

A timed-out handler is **not cancelled** — its promise keeps running in the background:

> A timed-out handler promise continues running because hook callbacks do not receive a
> timeout-owned cancellation signal. `before_tool_call` receives the owning tool call's
> `ctx.abortSignal`, but hook timeout expiry does not abort it.

**Missing binary / missing requirement (internal hooks only):** handled as an **eligibility
check performed before the hook is ever registered or run**, not a runtime failure — a hook
whose `requires.bins`/`requires.env`/`requires.config`/`requires.os` aren't satisfied is simply
excluded, visible via `openclaw hooks check` / `openclaw hooks info <name> -v` (shows a
"Missing" column). There's no documented "hook crashed because binary X was missing at
runtime" pathway — it's designed to never reach that state.

**Parallelism:** internal hooks — **NOT DOCUMENTED**. Typed plugin hooks — decision-capable
handlers run **sequentially**, descending priority; **observation-only handlers run in
parallel** and can overlap with later events (explicit warning not to rely on priority for
observation-side-effect ordering).

**Malformed output:** **NOT DOCUMENTED** for either surface — since there's no serialization
boundary (native JS return values), "malformed output" as a concept (bad JSON, wrong exit
code) doesn't apply the way it would to a subprocess hook. A handler returning the wrong
*shape* of object is a TypeScript/runtime-typing concern the docs don't address (no explicit
runtime schema validation of hook return values is mentioned).

---

## 9. Security posture

OpenClaw's security docs (`docs/gateway/security/index.md`) are extensive and explicit that
hooks and plugins are **arbitrary code execution by design**, quoted verbatim:

> ## Plugins
> Plugins run in-process with the Gateway - treat them as trusted code.
>
> - Only install from sources you trust; prefer explicit `plugins.allow` allowlists; review
>   plugin config before enabling; restart the Gateway after plugin changes.
> - Installing/updating plugins runs executable code:
>   - ClawHub packages and OpenClaw's bundled/official catalog are trusted sources. A new
>     arbitrary npm, `npm-pack:`, git, local path/archive, or marketplace source warns before
>     install; noninteractive installs require `--force` after you review and trust that
>     source. `--force` confirms provenance and permits overwrite; it does not bypass
>     `security.installPolicy` or remaining install safety checks.
>   - OpenClaw does not run built-in local dangerous-code blocking during install/update. Use
>     `security.installPolicy` for operator-owned local allow/block decisions and `openclaw
>     security audit --deep` for diagnostic scanning.
>   - Prefer pinned exact versions (`@scope/pkg@1.2.3`) and inspect the unpacked code before
>     enabling.

And on skills (adjacent, same trust model, worth noting since skills are grim's other
artifact kind for this client):

> Treat skill folders as trusted code and restrict who can modify them.

The priority-ordered triage list in the same doc explicitly puts plugin trust at position 5 of
6 ("Plugins: load only what you explicitly trust"), and the audit-finding catalog reserves a
dedicated `plugins.*`/`skills.*` **"supply chain"** check-id prefix and a separate `hooks.*`
prefix for **"per-surface hardening"** findings (`openclaw security audit`), e.g. dangerous
flags explicitly called out elsewhere in the same doc: `hooks.gmail.allowUnsafeExternalContent=true`,
`hooks.mappings[<index>].allowUnsafeExternalContent=true`.

There is **no interactive runtime prompt** ("this repo wants to run a hook, allow? y/n") the
way e.g. an editor extension host might show — the trust gate is at **install time**
(`--force` requirement for non-catalog sources, `plugins.allow`/`plugins.deny` allowlist) and
via an offline `openclaw security audit` command, not a just-in-time consent dialog per hook
invocation. The one exception is the typed `requireApproval` mechanism (§7), which is a
runtime human-in-the-loop gate, but it's opt-in policy the hook *author* chooses to add — not
something OpenClaw imposes on the hook itself.

No "config snapshotted at session start" language was found for hooks specifically; the
closest analogous concept is the Gateway's own last-known-good config snapshot (restored only
via `openclaw doctor --fix`, not automatically) and the general hot-reload watcher described
in §10.

---

## 10. Third-party installability — and the daemon-restart question

**Yes, an external tool can install an internal hook by writing files** — this is the most
grim-friendly of the three surfaces:

1. Write `~/.openclaw/hooks/<id>/HOOK.md` + `~/.openclaw/hooks/<id>/handler.js` (plain `.js`
   works; no TypeScript toolchain required).
2. Patch `hooks.internal.entries.<id>.enabled = true` (and `hooks.internal.enabled = true`)
   into `~/.openclaw/openclaw.json` (JSON5 — trailing commas/comments tolerated, but a strict
   installer can just emit clean JSON, which is valid JSON5).
3. That's the entire installation footprint — no build step, no compiler, no plugin manifest.

Installing a **typed plugin hook** is heavier: it requires a full plugin package (an
`openclaw.plugin.json` manifest with at minimum `id` and `configSchema`, plus a TypeScript ESM
entry module using `definePluginEntry`), installed via `openclaw plugins install <spec>` (npm
registry, ClawHub, or a local path/archive with `--force` if not from a trusted catalog). This
is a real package-manager-shaped install, not a drop-a-file operation.

**The daemon-restart question, with a real tension in the docs that I'm flagging rather than
resolving by guessing:**

- The *general* config hot-reload table in `docs/gateway/configuration.md` explicitly
  classifies the `hooks` config category as **not** requiring a restart:

  > | Automation | `hooks`, `cron`, `agent.heartbeat` | No (restarts that subsystem) |

  — i.e. editing `hooks.*` keys is described as hot-applying, restarting only the "hooks
  subsystem" in-process.

- But **both hook-specific docs pages say the opposite, twice, in direct operator-facing
  instructions**:
  - `docs/cli/hooks.md`: *"Restart the gateway after enabling (macOS menu bar app restart, or
    restart your gateway process in dev) so it reloads hooks."* / *"Sets
    `hooks.internal.entries.<name>.enabled = false`. Restart the gateway afterward."*
  - `docs/automation/hooks.md` troubleshooting section: *"Hook not executing: 1. Verify the
    hook is enabled ... 2. Restart your gateway process so hooks reload."*
  - And for plugins specifically (which is where typed hooks live):
    `docs/plugins/manage-plugins.md`: *"Installing or removing plugin code requires a Gateway
    restart."* And `docs/gateway/security/index.md`: *"...restart the Gateway after plugin
    changes."*

I read this as: the general reload-table row is describing config **value flips** for an
*already-discovered* hook subsystem (e.g. flipping a cron schedule or a heartbeat interval
live), while the **discovery of new hook files/directories on disk** — which is what a
third-party installer like grim actually does — is the part that's repeatedly, specifically,
and non-ambiguously documented as needing a Gateway restart. Given the brief's "exact strings
matter more than prose" rule, I'm not resolving this by picking one: **grim should assume a
restart is required after installing/removing a hook or plugin**, because that's what's stated
directly and repeatedly for exactly this operation, even though a more generic table elsewhere
suggests otherwise for bare config-value edits.

---

## 11. Trampoline viability

**Verdict: partially viable, with an architectural catch worth designing around.**

There is **no native "run this shell command" hook type** — every hook, of either kind, must
be a JS/TS module the Gateway's Node process actually `import`s and calls in-process. This
rules out the simplest trampoline shape (pointing OpenClaw config at an arbitrary executable
path the way e.g. a `command` string in some other client's hook config might work).

**However**, the module itself is free to shell out, and the docs' own example
(`gateway:pre-restart`, quoted in full in §6) does exactly that with
`node:child_process.execFile`. This means grim can still build a **generic trampoline**, just
one layer removed from what the brief's example command (`grim hook run --client openclaw
--event <E>`) implies:

- **For internal hooks:** grim materializes a tiny, identical-for-every-event
  `handler.js` (no compilation needed) whose entire body is: serialize `event` to JSON → spawn
  `grim hook run --client openclaw --event <event.type>:<event.action>` with that JSON on
  stdin → parse the child's stdout → if any strings come back, `event.messages.push(...)` them.
  `HOOK.md` frontmatter is generated per registered hook (name, `events` array, `requires`).
  This is **real and buildable today** — but its *output* ceiling is low: per §7, only 4 of the
  16 internal events (`command:new`, `command:reset`, `session:compact:before/after`) do
  anything with `event.messages` at all; everything else is fire-and-forget side-effect-only.
  So this trampoline is good for "run code on an event," not for "influence the agent's
  behavior."
- **For typed plugin hooks** (where the actual block/rewrite/approve power lives): the same
  execFile-and-parse trick works *inside* a plugin's `api.on(...)` handler, but grim would need
  to author and maintain an actual plugin package (manifest + entry module) as the shim, not
  just a config entry — heavier to materialize, and per §10 requires an explicit Gateway
  restart to load. The per-hook-kind typed return shape (different fields for
  `before_tool_call` vs `skill_proposal_evaluate` vs `resolve_exec_env`, etc.) also means a
  single generic `grim hook run` response schema would need a `--event`-scoped output contract
  per hook kind rather than one universal envelope — doable, but it's N schemas, not one.
- **Blockers, named directly:** (1) handler must be a JS/TS module OpenClaw imports, not an
  arbitrary binary path in config — grim must *write* the shim file, it cannot just *point at*
  `grim` in JSON; (2) no stdin/argv contract natively — the shim provides that itself via
  `child_process`; (3) typed-hook response must be a typed in-process object per hook kind,
  which the shim's `return` statement must construct from the subprocess's JSON, so grim needs
  one mapping per typed-hook name it wants to support, not a single pass-through; (4) a Gateway
  restart is needed after installing/updating either shim (§10) — grim's install flow can't be
  purely "drop a file and it's live."

---

## 12. Webhooks (inbound HTTP) — disambiguation detail, since the brief asked to flag this shape explicitly

This is **the opposite direction** from a lifecycle hook (external caller → OpenClaw, not
OpenClaw → external code), so it does not satisfy the brief's core "hook" definition, but it
*is* a genuine HTTP-callback-shaped extension point and worth recording precisely since the
brief explicitly asked to flag when the natural hook shape is an HTTP endpoint.

Enable via config (root `hooks.*`, a **different subtree from `hooks.internal.*`**):

```json5
{
  hooks: {
    enabled: true,
    token: "shared-secret",
    path: "/hooks"
  }
}
```

Auth: `Authorization: Bearer <token>` (recommended) or `x-openclaw-token: <token>` header;
query-string tokens explicitly rejected.

Endpoints:
- **`POST /hooks/wake`** — `text` (required), `mode` (`now`|`next-heartbeat`, default `now`),
  `agentId`. Enqueues a system event into the target agent's main session.
- **`POST /hooks/agent`** — `message` (required), plus `name`, `agentId`, `sessionKey`
  (requires `hooks.allowRequestSessionKey=true`), `sessionMode` (`isolated`|`persistent`),
  `idempotencyKey`, `wakeMode`, `deliver`, `channel`, `to`, `accountId`, `model`, `thinking`,
  `timeoutSeconds`. Runs an isolated (by default) agent turn. Response waits only for "runner
  admission" (up to 15s): `200`/`400`/`409`/`502`/`503`, `{ ok: false, error, runId }` on
  failure.
- **`POST /hooks/<name>`** — custom, resolved via a `hooks.mappings` **array** (not a map, note
  the shape difference from `hooks.internal.entries`) that transforms arbitrary payloads into
  `wake` or `agent` actions "with templates or code transforms."

Security warning, verbatim:

> Keep hook endpoints behind loopback, tailnet, or a trusted reverse proxy.
> - Use a dedicated hook token; do not reuse gateway auth tokens.
> - Keep `hooks.path` on a dedicated subpath; `/` is rejected.
> - Set `hooks.allowedAgentIds` to limit which effective agent a hook can target...

A separate, narrower `openclaw webhooks gmail setup|run` CLI exists purely for wiring Gmail
Pub/Sub push notifications into this same `/hooks/*` HTTP surface — not a general mechanism,
scoped to Gmail.

There's also a genuinely separate outbound-delivery "webhook" sense: Automations
(`docs/automation/index.md`) can **deliver** their output to "Channel, webhook, or silent" —
i.e. an automation's *result* can be POSTed to a URL. That's an outbound callback, but it's a
delivery-destination feature of the cron/automation subsystem, not a lifecycle-event hook
mechanism, and I did not find a config schema for it beyond the one-line mention in the
Automations-vs-Heartbeat comparison table — **NOT DOCUMENTED** in enough detail to say more.

---

## 13. Skills path context (for cross-checking the team lead's framing)

Confirmed from `docs/tools/skills-config.md`/`skills.md` search snippets (not fetched in full
raw form, so treat this paragraph as lower-confidence than the hook-specific sections above,
though it lines up with what the team lead's brief already asserted): skill load precedence is
`<workspace>/skills` → `<workspace>/.agents/skills` → `~/.agents/skills` → `~/.openclaw/skills`
→ bundled skills → `skills.load.extraDirs`. This matches the brief's claim that
`~/.openclaw/skills/` is the global shared root, and confirms OpenClaw's "workspace" is a
**per-agent** directory configured inside the single daemon config file — not a
cwd-discovered "project" the way grim's project scope works for CLI-style clients. That
supports grim's choice to model OpenClaw as global-only: the closest thing to "project scope"
(`<workspace>/hooks/`, `<workspace>/skills`) is still only reachable by editing the one daemon
config's `agents.entries.<id>.workspace` field, not by `grim install --workspace .` in some
directory a user `cd`s into.

---

## 14. Deprecations touching hooks (context, not load-bearing)

From `docs/plugins/hooks.md`'s "Upcoming deprecations" section and scattered `@deprecated`
tags: `subagent_spawning` (superseded by `subagent_spawned` with core-prepared `thread: true`
bindings), `api.registerSessionExtension(...)` (superseded, exact successor not fully quoted
here), `api.enqueueNextTurnInjection(...)` (deprecated alias), `timeoutBehavior` on
`requireApproval` (*"Unresolved approvals always deny"* regardless of the field's value now),
and `ctx.senderExternalId` (*"deprecated source-compatibility field"*). None of these change
the core contracts documented above; noted so grim doesn't accidentally build against a
surface already flagged for removal. Full removal timeline lives at
`/plugins/sdk-migration#removal-timeline` (not fetched — low priority for this brief).

---

## Sources

| URL | What it establishes | Fetched |
|---|---|---|
| `https://api.github.com/repos/openclaw/openclaw` (GitHub API) | Repo identity: org `openclaw`, homepage `openclaw.ai`, description, license field, size/star/fork counts, `pushed_at` recency, default branch `main` | 2026-08-14 |
| `raw.githubusercontent.com/openclaw/openclaw/main/docs/automation/hooks.md` | Internal hooks: full contract — event table, HOOK.md schema, discovery precedence, bundled hooks, config shape, restart guidance, troubleshooting | 2026-08-14 |
| `raw.githubusercontent.com/openclaw/openclaw/main/docs/plugins/hooks.md` | Typed plugin hooks: full catalog, `api.on` options, timeout table, `BeforeToolCallResult` schema, skill-proposal-evaluate schema, deprecations | 2026-08-14 |
| `raw.githubusercontent.com/openclaw/openclaw/main/docs/cli/hooks.md` | `openclaw hooks` CLI: enable/disable semantics, exact config keys mutated, restart requirement (verbatim, twice) | 2026-08-14 |
| `raw.githubusercontent.com/openclaw/openclaw/main/docs/gateway/configuration.md` | Config file path/format (JSON5, `~/.openclaw/openclaw.json`), `$include`, strict validation, hot-reload table (incl. the `hooks` row), reload modes | 2026-08-14 |
| `raw.githubusercontent.com/openclaw/openclaw/main/docs/help/environment.md` | **`OPENCLAW_HOME` definition** (precedence, semantics, example) — corrects the "never defined" prior note; also `OPENCLAW_CONFIG_PATH`, `OPENCLAW_STATE_DIR`, `OPENCLAW_INCLUDE_ROOTS` | 2026-08-14 |
| `repos/openclaw/openclaw/contents/src/config/paths.ts` (GitHub contents API) | Source-level confirmation of `OPENCLAW_HOME` handling (`resolveRequiredHomeDir`, native-service isolation check) | 2026-08-14 |
| `raw.githubusercontent.com/openclaw/openclaw/main/docs/gateway/security/index.md` | Security posture: "treat as trusted code" quote, install-time trust gate (`--force`, `plugins.allow`), audit check-id prefixes, dangerous-flag examples for `hooks.gmail.*`/`hooks.mappings` | 2026-08-14 |
| `raw.githubusercontent.com/openclaw/openclaw/main/docs/plugins/manage-plugins.md` | "Installing or removing plugin code requires a Gateway restart"; `--force`/trust flow for non-catalog installs | 2026-08-14 |
| `raw.githubusercontent.com/openclaw/openclaw/main/docs/plugins/plugin-permission-requests.md` | `requireApproval` routing detail, gate-selection table | 2026-08-14 |
| `raw.githubusercontent.com/openclaw/openclaw/main/docs/automation/cron-jobs.md` (Webhooks section, line ~529) | Inbound webhook HTTP endpoints, `hooks.mappings` array shape, auth header contract, security warning | 2026-08-14 |
| `raw.githubusercontent.com/openclaw/openclaw/main/docs/automation/index.md` | Extension-surface decision guide (hooks vs. automations vs. heartbeat vs. standing orders vs. task flow); "webhook" as an automation delivery target | 2026-08-14 |
| `raw.githubusercontent.com/openclaw/openclaw/main/docs/cli/webhooks.md` | Gmail-Pub/Sub-specific `openclaw webhooks` CLI (a narrow, unrelated-to-lifecycle-hooks surface) | 2026-08-14 |
| `repos/openclaw/openclaw/contents/package.json` (GitHub contents API) | Version `2026.8.1` (calendar versioning, not semver); Node engine range | 2026-08-14 |
| `docs.openclaw.ai/plugins/hooks`, `/automation/hooks`, `/help/environment`, openclaw.ai root | **All returned HTTP 403 to WebFetch** — attempted but unusable; recorded so the gap is explicit, not silent | 2026-08-14 |
| WebSearch: `OpenClaw AI agent daemon hooks plugins`, `"OpenClaw" multi-channel agent config skills ~/.openclaw` | Surfaced the doc site structure and, separately, several **[unofficial]** look-alike mirror domains (openclawlab.com, openclawcn.com, openclaw-ai.com, open-claw.bot, team400.ai, lumadock.com, stack-junkie.com, a cncfstack.com host) — none used as a source, flagged as leads only | 2026-08-14 |
