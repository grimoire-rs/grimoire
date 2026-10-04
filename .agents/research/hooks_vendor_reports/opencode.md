# OpenCode (sst/opencode → anomalyco/opencode) — hook / lifecycle-event mechanism

Research date: **2026-08-14**. Latest release at fetch time: **v1.18.18** (published
2026-08-13T01:15:04Z, per `gh api repos/anomalyco/opencode/releases`).

**Repo identity note:** the vendor's own docs at <https://opencode.ai/docs/> point at
`sst/opencode` in older material, but `gh api repos/sst/opencode` transparently redirects
(GitHub repo-rename redirect) to **`full_name: "anomalyco/opencode"`, `owner: "anomalyco"`**.
This is a real ownership move, not a fork (`is_fork: false`). All GitHub citations below use
the canonical `anomalyco/opencode` path; `sst/opencode` URLs still resolve to the same content
as of 2026-08-14.

---

## 1. Existence & name

OpenCode has exactly **one** hook/event mechanism: the **Plugin system** (vendor's own name;
docs title is literally "Plugins", <https://opencode.ai/docs/plugins/>). There is **no**
separate declarative/config-level "hook" surface — confirmed by direct inspection of the
authoritative config schema source (`packages/core/src/v1/config/config.ts` in
anomalyco/opencode, fetched 2026-08-14), which enumerates every top-level config key:

```
config, remote_config, shell, logLevel, server, command, skills, references, reference,
watcher, snapshot, plugin, share, autoshare, autoupdate, disabled_providers,
enabled_providers, model, small_model, default_agent, subagent_depth, username, mode,
agent, provider, mcp, formatter, lsp, instructions, layout (deprecated), permission,
tools, attachment, enterprise, tool_output, compaction, experimental
```

No `hook` or `hooks` key exists anywhere in this schema, nor in `experimental` (whose only
children are `batch_tool`, `openTelemetry`, `primary_tools`, `continue_loop_on_deny`,
`mcp_timeout`, `policies`). The only superficially adjacent key is
`autoupdate: boolean | "notify"` — an update-notification toggle, unrelated to event hooks.
**This proves the absence of a second, config-only hook mechanism**: the brief's hypothesis
of two distinct mechanisms (a config-level `hook`/`experimental.hook` key *and* a plugin
system) is wrong for OpenCode — there is only the plugin system.

- **Stability:** the Plugins doc page carries no beta/experimental banner; it's presented as
  a normal, stable feature. However, **6 of the ~19 named hook keys are individually
  prefixed `experimental.`** (see §3/§4) — stability is per-hook, not per-system.
- **Since when:** not pinned exactly (changelog archaeology was out of scope for the time
  box), but GitHub issue #5894 (filed 2025-12-21) already treats `tool.execute.before`
  plugins as an established, working feature at **OpenCode v1.0.182** — so the mechanism
  existed at least since early v1.0.x (Dec 2025 or earlier). NOT DOCUMENTED further back.
- **Deprecation:** none observed. Two old plugin npm packages
  (`opencode-openai-codex-auth`, `opencode-copilot-auth`) are explicitly deprecated because
  their functionality was absorbed into built-in plugins
  (`packages/opencode/src/plugin/shared.ts`, `DEPRECATED_PLUGIN_PACKAGES` — silently
  ignored if still configured, not an error).

---

## 2. Config location(s)

### Plugin *code* locations (verbatim from `packages/web/src/content/docs/plugins.mdx`)

> - `.opencode/plugins/` - Project-level plugins
> - `~/.config/opencode/plugins/` - Global plugins
>
> Files in these directories are automatically loaded at startup.

Format: **JavaScript or TypeScript modules** (`.js`/`.ts`), not JSON/YAML/TOML — this is a
code surface, not a data surface.

**Docs vs. source discrepancy (exact strings matter):** the docs page only ever shows the
**plural** directory name `plugins/`. The actual discovery glob, read verbatim from
`packages/opencode/src/config/plugin.ts`:

```ts
for (const item of await Glob.scan("{plugin,plugins}/*.{ts,js}", {
  cwd: dir, absolute: true, dot: true, symlink: true,
})) {
```

accepts **both `plugin/` (singular) and `plugins/` (plural)** — non-recursive, `.ts`/`.js`
only (not `.tsx`/`.mjs`/`.cjs` for the *directory scan*, though those extra extensions are
valid *entrypoints* once a target is otherwise resolved as an npm/path package — see
`INDEX_FILES` in `shared.ts`: `index.ts, index.tsx, index.js, index.mjs, index.cjs`).
Corroborated independently: GitHub issue #5894's reproduction (2025-12-21, v1.0.182) places
its plugin at `.opencode/plugin/test-guardrails.ts` (singular) and it loads and fires
correctly.

Separately, `packages/web/src/content/docs/config.mdx` states the general rule for **all**
`.opencode`-style subdirectories:

> The `.opencode` and `~/.config/opencode` directories use **plural names** for
> subdirectories: `agents/`, `commands/`, `modes/`, `plugins/`, `skills/`, `tools/`, and
> `themes/`. Singular names (e.g., `agent/`) are also supported for backwards compatibility.

**A third, undocumented-in-plugins.mdx location exists**, found only by reading
`packages/opencode/src/config/paths.ts`'s `directories()` function verbatim:

```ts
export const directories = Effect.fn("ConfigPaths.directories")(function* (directory, worktree) {
  const afs = yield* FSUtil.Service
  return unique([
    Global.Path.config,                                            // $XDG_CONFIG_HOME/opencode (~/.config/opencode)
    ...(!Flag.OPENCODE_DISABLE_PROJECT_CONFIG
      ? yield* afs.up({ targets: [".opencode"], start: directory, stop: worktree })  // every .opencode/ walking up to the git worktree root
      : []),
    ...(yield* afs.up({ targets: [".opencode"], start: Global.Path.home, stop: Global.Path.home })), // ~/.opencode  (bare, NOT the XDG config dir!)
    ...(Flag.OPENCODE_CONFIG_DIR ? [Flag.OPENCODE_CONFIG_DIR] : []),
  ])
})
```

Every directory in this list is passed to `ConfigPlugin.load(dir)` (plugin/plugins glob
scan) **and** — if it ends in `.opencode` or equals `OPENCODE_CONFIG_DIR` — also checked for
its own `opencode.json`/`opencode.jsonc`. So the full, source-verified location set for
auto-discovered plugin code is:

| Location | Scope | Source |
|---|---|---|
| `.opencode/plugin/` or `.opencode/plugins/`, at every level from cwd up to the git worktree root | project | docs + source |
| `~/.config/opencode/plugin/` or `~/.config/opencode/plugins/` (`$XDG_CONFIG_HOME/opencode` if set) | global | docs + source |
| `~/.opencode/plugin/` or `~/.opencode/plugins/` (bare home-relative, **not** the XDG dir) | global | source only — NOT in plugins.mdx |
| `$OPENCODE_CONFIG_DIR/plugin/` or `$OPENCODE_CONFIG_DIR/plugins/` | override | docs + source |

`Global.Path.config` itself is computed as `path.join(xdgConfig, "opencode")` using the
`xdg-basedir` npm package (`packages/core/src/global.ts`) — i.e. `$XDG_CONFIG_HOME/opencode`,
falling back to `~/.config/opencode`. This matches the brief's assumption exactly.

### Plugin *registration via config* (npm packages or explicit paths)

Config key `plugin` (array), in `opencode.json`/`opencode.jsonc`, verbatim doc example:

```json title="opencode.json"
{
  "$schema": "https://opencode.ai/config.json",
  "plugin": ["opencode-helicone-session", "opencode-wakatime", "@my-org/custom-plugin"]
}
```

Authoritative schema (`packages/core/src/v1/config/plugin.ts`):

```ts
export const Options = Schema.Record(Schema.String, Schema.Unknown)
export const Spec = Schema.Union([Schema.String, Schema.mutable(Schema.Tuple([Schema.String, Options]))])
```

So each array entry is **either a bare string** (npm package name, or a `file://`/relative/
absolute path) **or a 2-tuple `[string, options]`** where `options` is an arbitrary
`Record<string, unknown>` passed as the plugin function's second argument
(`Plugin = (input: PluginInput, options?: PluginOptions) => Promise<Hooks>`).

npm plugins are "installed automatically using Bun at startup. Packages and their
dependencies are cached in `~/.cache/opencode/node_modules/`." (plugins.mdx, verbatim).

### Config file locations (project/global), format, env vars, merge behavior

Format: **JSON and JSONC** only (no YAML/TOML for the main config; a **legacy TOML**
`~/.config/opencode/config` file is auto-migrated to `config.json` on load and then
deleted — see `config.ts` `loadGlobal()`).

Verbatim precedence order, `packages/web/src/content/docs/config.mdx`:

> 1. **Remote config** (from `.well-known/opencode`) - organizational defaults
> 2. **Global config** (`~/.config/opencode/opencode.json`) - user preferences
> 3. **Custom config** (`OPENCODE_CONFIG` env var) - custom overrides
> 4. **Project config** (`opencode.json` in project) - project-specific settings
> 5. **`.opencode` directories** - agents, commands, plugins
> 6. **Inline config** (`OPENCODE_CONFIG_CONTENT` env var) - runtime overrides
> 7. **Managed config files** (`/Library/Application Support/opencode/` on macOS) - admin-controlled
> 8. **macOS managed preferences** (`.mobileconfig` via MDM) - highest priority, not user-overridable
>
> :::note
> Configuration files are **merged together**, not replaced.
> :::

**Sources merge, they do not "one wins."** Confirmed identically in source (`config.ts`,
`mergeConfig`/`mergeConfigConcatArrays` called cumulatively for every source in order). For
the `plugin` array specifically, merging is a **union with de-dupe**, not concatenation with
duplicates: `ConfigPlugin.deduplicatePluginOrigins()` dedupes "on the load identity (package
name for npm specs, exact file URL for local specs)... later entries win" (comment, verbatim
from source) — i.e. **last-declared-wins per identity**, consistent with everything else in
OpenCode's config model ("last matching rule wins" is the recurring idiom — see permission
and policy sections below too).

Global config file: `~/.config/opencode/opencode.json` (tried in order `config.json` →
`opencode.json` → `opencode.jsonc` at the same directory, all merged).
Project config: `opencode.json`/`opencode.jsonc`, found by walking up from cwd to the
nearest `.git` (`ConfigPaths.files`, verbatim doc: "When OpenCode starts up, it first looks
for a config file in the current directory, then traverses up to the nearest Git directory.").

Env vars (all confirmed in source `Flag.*`):
- `OPENCODE_CONFIG` — path to one extra config file, merged between global and project.
- `OPENCODE_CONFIG_DIR` — extra `.opencode`-shaped directory (agents/commands/modes/
  plugins), loaded **after** global config and `.opencode` dirs, "so it **can override**
  their settings" (doc, verbatim).
- `OPENCODE_CONFIG_CONTENT` — raw JSON/JSONC text, merged last among the "normal" tiers.
- `OPENCODE_DISABLE_PROJECT_CONFIG` — skips project `opencode.json` discovery entirely
  (also skips the project `.opencode` walk-up for plugin directories — see `paths.ts`
  above).
- `OPENCODE_PERMISSION` — JSON deep-merged into the `permission` config key (not
  plugin-related, but same Flag-based override pattern).

Managed/MDM tiers (macOS `/Library/Application Support/opencode/`, Linux `/etc/opencode/`,
Windows `%ProgramData%\opencode`, plus macOS `.mobileconfig` under the
`ai.opencode.managed` preference domain) sit **above** everything, including project config
— admin-enforced, not user-overridable. These can, in principle, push a `plugin` array too
(same schema), which is relevant to "third-party installability" (§10): an org could force-
install a plugin org-wide via MDM, bypassing per-project consent entirely.

---

## 3. Config schema — verbatim; identity question

**The hook collection is an ARRAY, not a named map** — this differs from e.g. Claude Code's
`{"hooks": {"<event>": [...]}}` shape. `plugin` is `Array<string | [string, PluginOptions]>`.
There is no `{"hooks": {"PreToolUse": {...}}}`-style keyed structure anywhere in OpenCode's
config; the "map of event → handler" shape instead lives **inside the plugin's own return
value** (the `Hooks` object a plugin function returns), which is TypeScript-typed, not
JSON-configured.

### Stable identity for a third-party installer (critical for grim)

There is **no separate id/name/description config field** on a `plugin` array entry in the
common case. Identity for merge/dedupe purposes is derived, not declared:

- npm plugin → identity = **package name** (`parsePluginSpecifier(spec).pkg`)
- local/path plugin → identity = **exact resolved file URL**

verbatim from `packages/opencode/src/config/plugin.ts`:

```ts
export function deduplicatePluginOrigins(plugins: Origin[]): Origin[] {
  const seen = new Set<string>()
  const list: Origin[] = []
  for (const plugin of plugins.toReversed()) {
    const spec = pluginSpecifier(plugin.spec)
    const name = spec.startsWith("file://") ? spec : parsePluginSpecifier(spec).pkg
    if (seen.has(name)) continue
    seen.add(name)
    list.push(plugin)
  }
  return list.toReversed()
}
```

There IS a second, less-common plugin export convention (`packages/plugin/src/index.ts`,
type `PluginModule`) that **does** carry an explicit id:

```ts
export type PluginModule = { id?: string; server: Plugin; tui?: never }
```

...but this shape is only recognized in "detect" mode (`readV1Plugin(mod, spec, "server",
"detect")`) when the module's `default` export is an object containing `id`/`server`/`tui`
keys; **every doc example instead uses the plain-function legacy convention**
(`export const MyPlugin = async (ctx) => ({...hooks})`, no `id` at all — loader grabs it via
`Object.values(mod)` in `getLegacyPlugins()`). For file/path plugins using the *structured*
form, `id` is **required** (`resolvePluginId` throws `Path plugin ${spec} must export id` if
missing) — but that only applies to the structured form, not the common one.

**Practical implication for grim:** a third-party installer's stable identity for
add/update/remove-one-entry-idempotently is realistically **the file path or npm-spec
string grim itself writes into the `plugin` array / plugin directory** — not an in-band
`id` field. This is workable (grim already owns exact array entries and files for other
artifact kinds) but there's no vendor-native "installed-by" or "managed-by" marker field to
piggyback on.

### Matcher/filter syntax

Hooks are **not** filtered by a glob/regex matcher at the config layer at all — a plugin
function is loaded unconditionally and its returned `Hooks` object's keys ARE the filter
(e.g. only implement `"tool.execute.before"` to only care about tool calls). Fine-grained
filtering (by tool name) happens **inside the hook body**, reading `input.tool`, e.g. the
docs' own `.env` example: `if (input.tool === "read" && output.args.filePath.includes(".env"))`.
This is unlike Claude Code's `matcher` regex field — OpenCode has no declarative matcher at
all; it's 100% imperative JS inside the handler.

(Wildcard glob matching *does* exist elsewhere in OpenCode — the `permission` config's
tool/pattern rules use `*`/`?` glob matching, and `experimental.policies` resource IDs do
too — but neither is part of the plugin/hook config; see §9.)

---

## 4. Event catalogue

Two genuinely different mechanisms are bundled under the vendor's single "Events" heading
on the plugins doc page — **this distinction is my own synthesis from reading the full
`Hooks` TypeScript interface verbatim (`packages/plugin/src/index.ts`) against the docs'
undifferentiated event list**, and matters a lot for "can it block":

**(A) Named `Hooks` interface keys** — mutate `output` in place and/or throw to abort;
this is where actual interception happens. Exhaustive list, verbatim key names from source:

| Hook key | Fires | Can mutate | Can block? |
|---|---|---|---|
| `config` | plugin receives the resolved config once at startup | n/a (input only) | no |
| `event` | catch-all — every bus event, see (B) | no (input only) | no |
| `tool` | (not an event; registers custom tools) | — | — |
| `auth` | (not an event; registers an OAuth/API auth provider) | — | — |
| `provider` | (not an event; registers/extends a model provider) | — | — |
| `chat.message` | a new message is received | `output.message`, `output.parts` | not documented as denyable; throwing likely aborts (same propagation as tool.execute.before, not directly verified) |
| `chat.params` | before sending params to the LLM | `output.temperature/topP/topK/maxOutputTokens/options` | no |
| `chat.headers` | before sending HTTP headers to the provider | `output.headers` | no |
| `permission.ask` | a permission check is being resolved | `output.status: "ask"\|"deny"\|"allow"` | **yes — this is the explicit deny/ask/allow override** |
| `command.execute.before` | a `/slash-command` is about to run | `output.parts` | not documented; presumably throw-to-abort like tool hooks |
| `tool.execute.before` | before any tool call executes | `output.args` (in-place only, see §6) | **yes — throw to abort (documented pattern, §7)** |
| `tool.execute.after` | after a tool call executes (success path only — see §8) | `output.title/output/metadata` | no (already executed) |
| `shell.env` | env being prepared for a shell/bash execution or user terminal | `output.env` | no (additive only) |
| `tool.definition` | tool descriptions/params being sent to the LLM | `output.description`, `output.parameters` | no |
| `experimental.chat.messages.transform` | message list being assembled | `output.messages` | no |
| `experimental.chat.system.transform` | system prompt being assembled | `output.system` (string[]) | no |
| `experimental.provider.small_model` | resolving the "small model" | `output.model` | no |
| `experimental.session.compacting` | before compaction summary is generated | `output.context` (string[]) / `output.prompt` (full replace) | no |
| `experimental.compaction.autocontinue` | after compaction succeeds | `output.enabled` (skip synthetic continue turn) | yes (skips a downstream action) |
| `experimental.text.complete` | NOT DOCUMENTED beyond the type signature (`input: {sessionID, messageID, partID}`, `output: {text}`) | `output.text` | no |
| `dispose` | plugin/session teardown | n/a | no |

**(B) Generic event-bus values** (only reachable via the `event` hook,
`event: async ({event}) => { switch(event.type) {...} }`) — read-only, observational,
**verbatim from `plugins.mdx`**, grouped exactly as the doc groups them:

- **Command Events**: `command.executed`
- **File Events**: `file.edited`, `file.watcher.updated`
- **Installation Events**: `installation.updated`
- **LSP Events**: `lsp.client.diagnostics`, `lsp.updated`
- **Message Events**: `message.part.removed`, `message.part.updated`, `message.removed`, `message.updated`
- **Permission Events**: `permission.asked`, `permission.replied`
- **Server Events**: `server.connected`
- **Session Events**: `session.created`, `session.compacted`, `session.deleted`, `session.diff`, `session.error`, `session.idle`, `session.status`, `session.updated`
- **Todo Events**: `todo.updated`
- **Shell Events**: `shell.env` *(doc lists this here too, but it is ALSO a named, mutable Hooks key — see table A)*
- **Tool Events**: `tool.execute.after`, `tool.execute.before` *(same double-listing — these are named Hooks keys, not just passive events)*
- **TUI Events**: `tui.prompt.append`, `tui.command.execute`, `tui.toast.show`

I confirmed by reading the complete `Hooks` interface that **none of the (B) names outside
`tool.execute.before/after` and `shell.env` are members of `Hooks`** — `session.idle`,
`permission.asked`, `file.edited`, etc. are **only** observable passively through the
`event` catch-all, never as their own `(input,output)` mutation point. Mapping to the
brief's requested groups:

- **Session lifecycle**: `session.created/compacted/deleted/diff/error/idle/status/updated`
  (event-bus only) + `experimental.session.compacting`, `experimental.compaction.autocontinue`
  (named, mutable).
- **Prompt submit**: no single "UserPromptSubmit"-equivalent name exists. Closest are
  `chat.message` (fires for the message about to be sent, mutable) and
  `command.execute.before` (slash-commands only). `experimental.chat.messages.transform`,
  `experimental.chat.system.transform`, `tool.definition`, `chat.params`, `chat.headers`
  also intervene in the same pre-LLM-call window.
- **Pre/post tool use**: `tool.execute.before` / `tool.execute.after` (named, mutable/
  blocking) — the only fully-fledged before/after pair in the system.
- **File edit**: `file.edited`, `file.watcher.updated` (event-bus, post-hoc only — **no**
  `file.edit.before`/deny hook exists; the only way to gate an edit before it happens is the
  `permission` config's `edit`/`write`/`apply_patch` rules, or intercepting
  `tool.execute.before` when `input.tool` is one of those tool ids).
- **Command execution**: `command.executed` (event-bus, slash commands),
  `command.execute.before` (named), `shell.env` (named, shell/bash env injection).
- **Notification**: **no dedicated notify hook.** The documented pattern (plugins.mdx,
  "Send notifications" example, verbatim) is to implement the generic `event` hook and
  match on `event.type === "session.idle"`, then shell out:

  ```js title=".opencode/plugins/notification.js"
  export const NotificationPlugin = async ({ project, client, $, directory, worktree }) => {
    return {
      event: async ({ event }) => {
        if (event.type === "session.idle") {
          await $`osascript -e 'display notification "Session completed!" with title "opencode"'`
        }
      },
    }
  }
  ```
  Separately (out of scope, disambiguating per the brief): the **OpenCode desktop app**
  (a different client surface) "can send system notifications automatically when a response
  is ready or when a session errors" — a built-in feature, not a hook.
- **Stop/finish**: `session.idle` (event-bus) is the closest analog; no blocking
  "can-it-stop" hook exists.
- **Compaction**: covered above under session lifecycle.
- **Subagent**: **NOT DOCUMENTED as its own category** — no `subagent.*` event/hook name
  exists. See §8 for how subagent tool calls interact with `tool.execute.before`.
- **Error**: `session.error` (event-bus) only. `tool.execute.after` does **not** fire on a
  failed tool call (see §8) — there is no distinct "tool error" hook.

---

## 5. Invocation

**In-process JS function calls — never a subprocess, never stdin/stdout, never HTTP.**
Confirmed exhaustively by reading `packages/opencode/src/plugin/index.ts`: a plugin module
is `import()`-ed once at startup (dynamic ESM import of a file:// URL or resolved npm
entrypoint), its exported function(s) are called once to produce a `Hooks` object, and that
object is kept in an in-memory array (`state.hooks: Hooks[]`) for the life of the OpenCode
process/instance. Every subsequent event is a **direct JS function call**:

```ts
const trigger = Effect.fn("Plugin.trigger")(function* (name, input, output) {
  if (!name) return output
  const s = yield* InstanceState.get(state)
  for (const hook of s.hooks) {
    const fn = hook[name] as any
    if (!fn) continue
    yield* Effect.promise(async () => fn(input, output))
  }
  return output
})
```

- **Sequential, not parallel.** Hooks of the same name run one after another, in plugin
  **registration order** (which is itself deterministic: internal built-in auth plugins
  first, then external plugins in the order the loader resolved them — the source comment
  is explicit: `// Keep plugin execution sequential so hook registration and execution
  order remains deterministic across plugin runs.`).
- **Shared mutable `output` object.** The *same* object reference is passed to every plugin
  registered for that hook name — plugin 2 sees whatever mutations plugin 1 already made to
  `output`. This is why the docs' shell-escaping example mutates a *property* of `output.args`
  (`output.args.command = escape(output.args.command)`) rather than reassigning
  `output.args` wholesale — see §6 for why that distinction matters.
- **No visible timeout.** `Effect.promise(async () => fn(input, output))` has no
  `.pipe(Effect.timeout(...))` wrapping in the code I read. NOT DOCUMENTED as a limit
  anywhere in the docs either. A hook that never resolves its promise will hang that event
  indefinitely (see §8).
- **Working directory / shell / $PATH:** the plugin function itself runs inside the OpenCode
  Node/Bun process — no separate cwd/shell/PATH question applies (it's not spawned). It is
  handed `directory` (project root) and `worktree` (git worktree path) as plain strings in
  `PluginInput`, and `$` — **Bun's shell API** (`Bun.$`) — for the plugin's *own* use if it
  wants to run subprocesses (`$` is explicitly typed `BunShell` and is `undefined` when
  `typeof Bun === "undefined"`, i.e. plugins are Bun-first; behavior under a pure-Node
  runtime is NOT DOCUMENTED beyond that guard existing).
- **Plugin loading itself IS async/concurrent** (`Promise.all` over candidates in
  `PluginLoader.loadExternal`), but that's startup resolution, not per-event invocation.

---

## 6. Input payload — verbatim

**No stdin/env/argv/template-string payload delivery at all.** Every hook receives its data
as **two native JS objects passed as function arguments**: `(input, output)`. `input` is
read-mostly context; `output` is the mutable object the hook is expected to edit. Exact
shapes, verbatim from `packages/plugin/src/index.ts`:

```ts
"tool.execute.before"?: (
  input: { tool: string; sessionID: string; callID: string },
  output: { args: any },
) => Promise<void>

"tool.execute.after"?: (
  input: { tool: string; sessionID: string; callID: string; args: any },
  output: { title: string; output: string; metadata: any },
) => Promise<void>

"permission.ask"?: (input: Permission, output: { status: "ask" | "deny" | "allow" }) => Promise<void>

"shell.env"?: (
  input: { cwd: string; sessionID?: string; callID?: string },
  output: { env: Record<string, string> },
) => Promise<void>

"chat.message"?: (
  input: { sessionID: string; agent?: string; model?: {providerID,modelID}; messageID?: string; variant?: string },
  output: { message: UserMessage; parts: Part[] },
) => Promise<void>
```

Real payload-shape example from the docs (`.env` protection plugin):

```javascript
"tool.execute.before": async (input, output) => {
  if (input.tool === "read" && output.args.filePath.includes(".env")) {
    throw new Error("Do not read .env files")
  }
}
```

**Mutation is by reference, in place — reassignment does not propagate.** I traced the real
call site (`packages/opencode/src/session/tools.ts`):

```ts
yield* plugin.trigger(
  "tool.execute.before",
  { tool: item.id, sessionID: ctx.sessionID, callID: ctx.callID },
  { args },                       // <- output.args IS the same object as the outer `args`
)
const result = yield* item.execute(args, ctx)   // <- reads the outer `args` variable, NOT the trigger's return value
```

Because `{ args }` is a shorthand property that aliases the *same* object, a hook doing
`output.args.command = "…"` (mutating a field) is visible to `item.execute(args, ctx)`
afterward. A hook doing `output.args = {...somethingNew}` (reassigning the property) would
**not** be visible, because `execute()` never reads back `plugin.trigger`'s return value —
it keeps using its own closed-over `args` variable. This is a real, exact-strings gotcha,
not a hypothetical: any wrapper (including a future grim shim) MUST mutate nested fields of
the object it's handed, never replace the whole object.

There is **no** documented JSON-on-stdin schema, **no** env-var payload convention (aside
from `shell.env`'s `output.env`, which is a *result*, not an input-delivery channel), and
**no** `$TOOL_NAME`/`{{file}}`-style string templating anywhere in the hook contract.

---

## 7. Output / response contract — verbatim

**There is no exit-code contract at all** — hooks are JS functions, not subprocesses, so
"exit code 0/1/2" has no meaning here (unlike Claude Code's shell-hook model). The two real
response channels are:

1. **Mutate `output` in place**, per the field tables in §4/§6 (deny/allow via
   `permission.ask`'s `output.status`, args-rewrite via `tool.execute.before`'s
   `output.args`, env injection via `shell.env`'s `output.env`, etc.).
2. **Throw** (`throw new Error(...)`) to abort. This is the **documented, official** way to
   block a tool call — verbatim from `plugins.mdx`'s ".env protection" example:

   ```javascript title=".opencode/plugins/env-protection.js"
   export const EnvProtection = async ({ project, client, $, directory, worktree }) => {
     return {
       "tool.execute.before": async (input, output) => {
         if (input.tool === "read" && output.args.filePath.includes(".env")) {
           throw new Error("Do not read .env files")
         }
       },
     }
   }
   ```

   Tracing what actually happens on that throw (`plugin/index.ts`'s `trigger` uses
   `Effect.promise(async () => fn(input, output))` with **no `catch`**): a rejected promise
   inside `Effect.promise` becomes an **Effect "defect"** (an unrecovered exception), which
   propagates up through the `Effect.gen` block in `session/tools.ts`'s tool `execute()`
   wrapper — aborting that Effect chain before `item.execute(args, ctx)` (the real tool
   body) ever runs. The surrounding code has no defect-catching (`Effect.catchAllDefect`,
   etc.) at that specific call site, so this becomes a rejected Promise for the Vercel AI
   SDK's tool `execute()` — which the AI SDK will typically surface as a tool-call error
   part back to the model/session, not a silent hang or a process crash. **This is
   inference from reading the exact control flow, not a vendor-documented exit contract** —
   there is no field like `{"decision":"block"}` anywhere; "block" == "throw and let the
   error propagate as the tool's failure."
   - There is NO evidence this exception path also fires `tool.execute.after` — see §8:
     `tool.execute.after`'s trigger call is textually *after* `item.execute(...)` in a
     sequential `Effect.gen`, so an exception at or before that point (including one thrown
     from `tool.execute.before` itself) skips the `tool.execute.after` trigger entirely.
3. **`dispose`** returns `Promise<void>`, no output object — pure teardown notification.
4. **stdout/stderr**: not applicable — no subprocess, so there's no separate "shown to user
   vs shown to model" stdout/stderr split as in shell-hook clients. Whatever a plugin wants
   surfaced should go through `client.app.log()` (doc-recommended, verbatim: "Use
   `client.app.log()` instead of `console.log` for structured logging"; levels `debug`,
   `info`, `warn`, `error`) or by mutating a `parts`/`message` field that the UI already
   renders (e.g. `chat.message`'s `output.parts`).

---

## 8. Reliability & limits

- **Timeout:** NOT DOCUMENTED, and none found in the `trigger()` implementation I read
  (§5). No default, no override flag.
- **Non-zero exit:** not applicable (no subprocess/exit codes); the equivalent failure mode
  is an unhandled promise rejection/throw, discussed in §7.
- **Malformed output:** if a hook sets `output.status` (for `permission.ask`) to something
  other than the three literal union values, or leaves required fields with the wrong type,
  behavior is NOT DOCUMENTED — TypeScript types are compile-time only; nothing in the
  runtime `trigger()` code validates `output`'s shape after a hook runs.
- **Missing binary:** not applicable to hook invocation itself (no subprocess spawn for the
  hook call). It IS applicable to plugin *loading*: a missing/failed npm install or a
  missing entrypoint is caught and reported through `PluginLoader`'s `report.error()`
  callback (`install`/`entry`/`compatibility`/`load` stages), which publishes a
  `Session.Event.Error` — i.e. **plugin load failures degrade gracefully** (the plugin is
  just skipped, an error event is published, OpenCode keeps running) rather than crashing
  the whole process. Verbatim from `plugin/index.ts`'s error branches:
  `Failed to install plugin ${pkg}@${version}: ...`, `Plugin ${spec} skipped: ...`,
  `Failed to load plugin ${spec}: ...`.
- **Parallel or sequential:** sequential across plugins for the same hook name (§5); no
  documented concurrency control or ordering guarantee beyond "registration order," and
  registration order itself is only loosely documented (plugins.mdx "Load order": global
  config → project config → global plugin dir → project plugin dir — but this is the order
  *sources* are scanned, not a guarantee about ordering *within* a source when multiple
  files/exports exist there).
- **Blocking vs fire-and-forget:** **blocking** for every named `Hooks` key — the
  `Effect.gen` sequence `yield*`s the trigger before continuing, so the tool call (or
  chat-param resolution, etc.) genuinely waits on the hook's promise. The generic `event`
  hook is dispatched differently and is closer to fire-and-forget:
  ```ts
  const unsubscribe = yield* events.listen((event) => {
    if (event.location?.directory !== ctx.directory) return Effect.void
    return Effect.sync(() => {
      for (const hook of hooks) {
        void hook["event"]?.({ event: {...} })   // <- `void`: rejection is NOT awaited/caught
      }
    })
  })
  ```
  Note the `void` before the call — the `event` hook's promise is explicitly **not**
  awaited, so a slow or throwing `event` handler cannot block or crash anything; it's true
  fire-and-forget. This is a real, source-verified asymmetry between the two mechanisms in
  (A) vs (B) of §4.
- **Known reliability gap, source-confirmed via GitHub issue investigation:** the
  `experimental.batch_tool` (config flag, batches multiple tool calls into one LLM turn)
  has its own execution path that — per a collaborator's investigation on issue #5894 —
  "calls `tool.execute()` directly without going through `Plugin.trigger()`", i.e. **tool
  calls routed through the batch tool skip `tool.execute.before`/`tool.execute.after`
  entirely.** Quoted from the issue thread (ArmirKS, 2026, commenting on
  anomalyco/opencode#5894): "batch.ts calls tool.execute() directly without going through
  Plugin.trigger(). so any tools [called through it bypass hooks]". I did not independently
  re-verify this against `batch.ts` source in this pass (time-boxed), but it is consistent
  with everything else observed about how ad hoc, per-call-site the `plugin.trigger(...)`
  wiring is (§5's `session/tools.ts` shows it manually inserted at every tool call site —
  nothing in the architecture *guarantees* every current or future tool-execution path
  remembers to call it).
- **Subagent tool calls DO fire hooks** (contrary to the original bug report in #5894):
  "When the task tool spawns a subagent it runs its own session through the same prompt
  loop and plugins are loaded per-Instance so the hooks DO fire for the subagent's tools."
  (ArmirKS, anomalyco/opencode#5894). The original reporter's "bypass" was actually the
  subagent invoking `grep`/`glob` *via the `bash` tool*, so `tool.execute.before` correctly
  fired with `input.tool === "bash"`, not `"grep"` — a modeling/UX confusion, not a real
  hook gap. Worth noting for grim: a hook filtering on tool id has to account for the model
  routing around a specific tool name via a more general one it also has access to.

---

## 9. Security posture

**No trust/approval/allowlist gate exists for local plugin code**, and this is a
vendor-acknowledged (via engaged maintainers/collaborators, though not yet shipped) gap, not
speculation:

- GitHub issue **anomalyco/opencode#6361**, "No trusted workspace functionality leads to
  arbitrary commands execution on startup" (filed 2025-12-29 against v1.0.207, labeled
  `bug`, auto-closed 90 days later for inactivity — **not** marked fixed/resolved). Verbatim
  from the report: "OpenCode automatically trusts and executes MCP server commands from
  local `opencode.json` without user consent. This allows arbitrary command execution when
  a user opens OpenCode in a malicious repository." And explicitly, under "Additional Attack
  Surfaces": **"Same issue applies to local plugins."** A repo maintainer/contributor
  (`Mishkun`, tagging `@thdxr`) proposed reusing the existing permission-dialog UI for a
  "trust this workspace" gate (`allow for this config` / `always allow` / `deny and exit`),
  but flagged the real blocker themselves: **"as I scout the codebase, config and plugins
  loaded before any gui. How do we get around that?"** — i.e. plugin code executes
  (top-level module evaluation, at minimum) **before** any prompt could even be shown.
  I did not find any later doc page (`plugins.mdx`, `permissions.mdx`, `policies.mdx`, all
  fetched 2026-08-14) describing a trust prompt that would supersede this — so as of
  v1.18.18 this appears to remain unaddressed, though I did not exhaustively diff every
  release's changelog to be certain.
- A related, already-**fixed** vulnerability for context (NOT plugin-specific, don't
  conflate): **GHSA-vxw4-wv6m-9hhh**, "Unauthenticated HTTP Server Allows Arbitrary Command
  Execution," CVSS 8.8, fixed in v1.0.216. This is about OpenCode's local HTTP server
  (`POST /session/:id/shell`, `POST /pty`, permissive CORS) having no auth — a different
  attack surface than plugins, but it shows the vendor does ship security fixes and use
  GHSA advisories when a report is accepted, which makes #6361's 90-day stale-close (rather
  than a GHSA) meaningfully different — it was not escalated/fixed the same way.
- The `permission` config (`ask`/`allow`/`deny` per tool, glob-matched) is a **separate**
  mechanism from plugin trust — it governs whether *OpenCode's own tool calls* need
  approval, not whether a repo's `.opencode/plugins/*.js` is allowed to run its own
  arbitrary top-level code at all. Default posture, verbatim from `permissions.mdx`: most
  permissions default to `"allow"`; only `doom_loop`, `external_directory` default to
  `"ask"`, and `.env` file reads default to `"deny"`. None of this gates plugin loading.
- `experimental.policies` (config `experimental.policies[]`, `{effect, action, resource}`,
  currently only supporting `action: "provider.use"`) is explicitly scoped to LLM-provider
  access, not to code execution or hooks — verbatim from `policies.mdx`: "Permissions
  control what tools can do during a session, while policies control whether OpenCode may
  use a resource such as an LLM provider." Not a hook-trust mechanism.

---

## 10. Third-party installability

**Yes, straightforwardly, by editing files** — no vendor CLI required to *install* a
plugin:

- Drop a `.js`/`.ts` file into `.opencode/plugin(s)/` (project) or
  `~/.config/opencode/plugin(s)/` (global) — auto-discovered at next startup, per §2/§5. No
  registration step, no config edit needed for this path.
- Or append an entry to the `plugin` array in `opencode.json`/`opencode.jsonc` (a plain
  JSON/JSONC array edit — exactly the kind of "splice a native config file in place" grim
  already does for other clients) for an npm-package or explicit-path plugin.

**"Config snapshotted at startup" gotcha: yes, confirmed.** Plugins (and indeed the whole
merged config) are loaded once during `Config.Service`'s instance-state initialization
(`InstanceState.make(...)` in `config.ts`/`plugin/index.ts`) and cached
(`Effect.cachedInvalidateWithTTL(..., Duration.infinity)` for the global config layer). **A
newly-dropped plugin file or a newly-added `plugin` array entry requires an OpenCode
restart** (new process / new `opencode` invocation) to take effect — there is no
documented hot-reload for plugin files, unlike some editors' extension systems. (This is my
inference from the caching/init structure, matching the general shape of every
config-loading client I've seen documented in this project — plugins.mdx and config.mdx do
not explicitly state a restart requirement, but nothing in either doc claims live-reload,
and the code visibly loads plugins once as part of a cached, TTL-`infinity` service
bootstrap.)

**No UI-only or cloud-only gate** blocks a file-based install: the desktop app, TUI, and
headless CLI all read the same `opencode.json`/`.opencode` directory tree.

---

## 11. Trampoline viability

**A single generic command IS viable for the two "hard" named hooks
(`tool.execute.before`/`.after`, `permission.ask`) with real caveats; it is NOT viable for
`event` (fire-and-forget, no response channel to act on) without accepting it becomes purely
observational.**

What a `grim hook run --client opencode --event <E>` trampoline would need, concretely:

1. **A generated JS/TS shim file is mandatory** — OpenCode never shells out for a hook by
   itself; the *only* invocation path is `import()` + direct function call (§5). Grim would
   have to materialize a real `.opencode/plugins/grim-<name>.js` (or `.ts`) file whose
   exported function, when called by OpenCode, itself shells out to `grim hook run ...`,
   waits for it to finish, parses its stdout as JSON, and **mutates the `output` object
   in place** (never reassigns it — §6) with whatever the subprocess reported. This is
   exactly the same shape grim already uses conceptually for other vendors' JS-plugin
   surfaces, and it is mechanically sound — the aliasing problem in §6 is a real constraint
   on the shim's *implementation*, not a blocker on the *approach*.
2. **The shim, not grim's binary, is what OpenCode actually calls.** Grim's own process
   model (spawn `grim`, get stdin/stdout back) is preserved *inside* the shim — OpenCode
   itself never sees a subprocess boundary, so none of OpenCode's own timeout/concurrency
   guarantees apply (because there aren't any — §8). The shim inherits whatever timeout
   grim wants to enforce on its own subprocess call; nothing in OpenCode will kill a
   hanging shim for you.
3. **Blocking a tool call has no first-class field** — the shim must `throw` to deny (§7),
   which OpenCode surfaces as a tool-execution error, not a clean "denied" UI state. A
   `grim hook run` exit-code/JSON contract of "deny" would need the shim to translate that
   into `throw new Error(payload.reason)`, losing structure (no `permission.ask`-style
   allow/ask/deny trichotomy for `tool.execute.before` — only binary throw-or-don't).
   `permission.ask` is the one hook with a real three-state `output.status` field, so a
   deny/ask/allow trampoline maps cleanly **only** for that one hook.
4. **Auto-discovery is a free win** — because `.opencode/plugin(s)/*.{ts,js}` is
   auto-loaded with zero config-file edits (§2), grim could materialize the shim file alone
   (no `opencode.json` splice needed) for the local-file install path, which is simpler than
   most other clients' JSON-hook surfaces. The npm-array (`plugin: [...]`) path is also
   available if grim ever wants to ship the shim as a versioned npm package instead of a
   loose file.
5. **Blockers, named plainly:**
   - No stdin delivery convention exists — the shim has to serialize `input`/`output`
     itself (e.g. `JSON.stringify` to argv or a temp file) when calling out to `grim hook
     run`; there is no vendor-native "hooks get JSON on stdin" contract to lean on (unlike,
     reportedly, some other clients).
   - `output` mutation-by-reference (§6) means the shim must deep-merge the subprocess's
     JSON response back onto the *original* object fields, not replace top-level
     properties, for every hook where nested mutation matters (`tool.execute.before`'s
     `output.args` in particular).
   - No timeout/kill guarantee from OpenCode (§8) — a hung `grim hook run` process hangs
     the user's tool call indefinitely unless the shim itself imposes a timeout.
   - `tool.execute.after` never fires on a failed tool call (§7/§8) — a trampoline can't
     rely on it as a universal "tool finished" signal.
   - The generic `event` catch-all cannot be trampolined into anything but pure logging/
     notification — its promise is explicitly `void`-discarded (§8), so no response from
     `grim hook run` could ever be read back for it even if the shim tried.
   - No restart-free hot reload (§10) — every shim (re)deployment needs the user to restart
     `opencode` to take effect, same as adding any other plugin.

---

## Sources

| URL | What it establishes | Fetched |
|---|---|---|
| https://opencode.ai/docs/plugins/ | Vendor doc: plugin locations, load order, events list, all code examples quoted verbatim (fetched raw source, not summarized) | 2026-08-14 |
| https://opencode.ai/docs/config/ | Vendor doc: config precedence, locations, env vars, plural/singular subdir note | 2026-08-14 |
| https://opencode.ai/docs/permissions/ | Vendor doc: permission config schema, defaults, approval-prompt UX | 2026-08-14 |
| https://opencode.ai/docs/policies/ | Vendor doc: `experimental.policies`, scoped to provider access only | 2026-08-14 |
| https://opencode.ai/config.json | Published JSON Schema for `opencode.json`; corroborates `plugin`/`experimental` shape, no `hook` key | 2026-08-14 |
| https://github.com/anomalyco/opencode (`gh api repos/sst/opencode`) | Confirms `sst/opencode` → `anomalyco/opencode` rename/redirect, not a fork | 2026-08-14 |
| `packages/plugin/src/index.ts` (anomalyco/opencode, branch `dev`) | Full, verbatim `Hooks` TypeScript interface — every named hook key, input/output shape | 2026-08-14 |
| `packages/opencode/src/plugin/index.ts` (anomalyco/opencode) | `trigger()` invocation engine: sequential, shared mutable output, no timeout, `event` hook is fire-and-forget (`void`) | 2026-08-14 |
| `packages/opencode/src/plugin/loader.ts`, `shared.ts` | Plugin resolution/loading pipeline; error handling degrades gracefully; two export conventions (legacy function vs. `{id,server,tui}`) | 2026-08-14 |
| `packages/opencode/src/config/plugin.ts` | Directory-scan glob `{plugin,plugins}/*.{ts,js}`; dedupe-by-identity merge logic | 2026-08-14 |
| `packages/opencode/src/config/config.ts` | Full config source-merge order, env-var flags, plugin_origins provenance tracking | 2026-08-14 |
| `packages/opencode/src/config/paths.ts`, `packages/core/src/global.ts` | Authoritative directory list for plugin/config discovery incl. undocumented `~/.opencode`; XDG resolution | 2026-08-14 |
| `packages/opencode/src/session/tools.ts` | Real call site of `plugin.trigger("tool.execute.before"/"after", ...)`; proves after-hook skipped on throw; proves in-place-mutation-only aliasing | 2026-08-14 |
| `packages/opencode/src/permission/index.ts` | Config-driven `permission` ask/allow/deny engine (distinct from the `permission.ask` plugin hook) | 2026-08-14 |
| `packages/core/src/v1/config/config.ts`, `plugin.ts` | Authoritative Effect-Schema config type: exhaustive top-level key list, proves no `hook`/`hooks` key exists | 2026-08-14 |
| https://github.com/anomalyco/opencode/issues/6361 | "No trusted workspace functionality..." — vendor-side (contributor-engaged) acknowledgment that plugins execute with zero consent gate; stale-closed, not fixed | 2026-08-14 |
| https://github.com/anomalyco/opencode/security/advisories/GHSA-vxw4-wv6m-9hhh | Fixed, unrelated (HTTP server) RCE advisory — context for vendor's security-fix posture, explicitly NOT about plugins | 2026-08-14 |
| https://github.com/anomalyco/opencode/issues/5894 | `tool.execute.before` subagent-bypass report; refuted by collaborator investigation; surfaces genuine `batch.ts` hook-skip gap | 2026-08-14 |
| https://github.com/sst/opencode/issues/3367 | `[unofficial-adjacent but primary]` closed feature request for an `assistant.output.beforeParse` hook — proves the named-hook surface is still actively negotiated/extended, and that this specific hook does NOT exist today | 2026-08-14 |
| `gh api repos/anomalyco/opencode/releases` | Latest version at fetch time: v1.18.18, published 2026-08-13 | 2026-08-14 |
