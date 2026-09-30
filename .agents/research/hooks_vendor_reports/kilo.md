# Kilo Code — hook / lifecycle-event mechanism

**Client:** Kilo (formerly "Kilo Code"; org `Kilo-Org`, repo `kilocode`). VS Code extension,
JetBrains plugin, CLI (`@kilocode/cli`, binary `kilo`), and "Cloud Agents". Descends from
Roo Code / Cline, but **as of 2026-04-02 the whole product line was rebuilt on top of
OpenCode's (SST) portable server/runtime** — this is the single most important fact for
this research: Kilo's current hook mechanism is not a Cline-lineage feature at all, it is
**OpenCode's plugin/hook system, ported and lightly rebranded**, and the docs and source
still leak the word "opencode" in many exact strings (env vars, cache paths, engine-compat
keys, file names).

Today's date: 2026-08-14. All fetches below are dated 2026-08-14 unless noted.

---

## 0. IMPORTANT correction to the brief's assumed paths

The brief states: *"Known grim-relevant paths: global `~/.kilo` or `$XDG_CONFIG_HOME/kilo`"*.

Every current official source I found (rendered docs, raw doc source, CLI `--help` surface
description) says the global directory is **`~/.config/kilo/`** — i.e. XDG-style
(`$XDG_CONFIG_HOME/kilo`, falling back to `~/.config/kilo`), **not** a bare `~/.kilo`. Quote,
from the CLI docs (`packages/kilo-docs/pages/code-with-ai/platforms/cli.md`, raw, fetched
2026-08-14):

> | Scope | Path |
> |---|---|
> | **Global** | `~/.config/kilo/kilo.json[c]` or legacy `opencode.json[c]` (Windows config dir may vary) |
> | **Project** | `./kilo.json[c]`, legacy `./opencode.json[c]`, or config inside `./.kilo/` (legacy `./.kilocode/` is also read) |

Plugin auto-discovery directories are stated the same way in the Plugins doc: **`~/.config/kilo/plugin/`** (global), `.kilo/plugin/` or legacy `.kilocode/plugin/` (project). I did not find `~/.kilo` (no leading `.config`) anywhere in current docs or source. Recommend
double-checking grim's actual implementation against `~/.config/kilo/` before shipping a
hook materializer for this client — if grim currently writes to bare `~/.kilo`, that may
already be a latent bug independent of this hooks project.

The project-scope legacy path `.kilocode/` is confirmed **still actively read**, not merely
detected: it's an explicit fallback in the plugin auto-discovery list and the "standalone
tool files" convention. That matches the brief's framing of `.kilocode` as legacy-but-present.

---

## 1. Existence & name

**Exists: YES.** Vendor's own term is **"Plugins"** (page title), and the individual
extension points inside a plugin are literally called **"hooks"** — H2 "Hooks reference" —
in the same doc. Frontmatter of the doc (raw markdown):

```
---
title: "Plugins"
description: "Extend the Kilo CLI with custom hooks, tools, auth providers, and more"
platform: new
---
```

**Status:** Not labeled beta/experimental as a whole. Applies to **"Kilo version 1.0 and
later"** (CLI doc: *"This documentation applies only to Kilo version 1.0 and later. Users
running versions below 1.0 should upgrade before proceeding."*). A clearly-marked
**experimental subset** exists: hooks under the `experimental.` prefix ("may change between
releases") and the `experimental_workspace` plugin-input field ("This API is experimental
and may change").

**Since which version / timeline** (reconstructed from git history of
`Kilo-Org/kilocode`, fetched via `gh api`, 2026-08-14):

| Date | Event | Source |
|---|---|---|
| 2026-04-02 | Kilo Code for VS Code "completely rebuilt" on OpenCode server, GA. "Kilo now shares the same engine across VS Code, the CLI, and Cloud Agents." | blog.kilo.ai/p/new-kilo-for-vs-code-is-live (author: Job Rietbergen) |
| 2026-04-24 | GitHub issue `Kilo-Org/kilocode#9476` filed: "Add OpenCode-Compatible Runtime Plugin Support in Kilo" — reports no visible `kilo plugin` command, unclear where plugins go | github.com/Kilo-Org/kilocode/issues/9476 |
| 2026-04-27 | Commit `6a32cb1e` "docs(kilo-docs): clarify plugin cache path" — plugin docs already existed by this date | `gh api repos/Kilo-Org/kilocode/commits?path=packages/kilo-docs/pages/automate/extending/plugins.md` |
| 2026-06-09 / 06-18 / 07-13 | Commits "refactor: kilo compat for v1.14.51" / "for v1.15.13" / "OpenCode v1.16.2 (#12088)" touching `packages/plugin/src/index.ts` — Kilo's plugin **type package is mechanically synced from upstream OpenCode point releases** | same repo, path `packages/plugin/src/index.ts` |
| 2026-06-24 | Issue #9476 **auto-closed by a stale-issue bot** after 60 days of inactivity. GitHub API `stateReason: "NOT_PLANNED"` was applied automatically by that bot flow — **no maintainer ever commented on the substance**. Only comment: *"To stay organized issues are automatically closed after 60 days of no activity. If the issue is still relevant please reopen it or create a fresh new one."* (author: `github-actions` bot) | `gh issue view 9476 --json ...` |
| 2026-06-24 | Same-day commit `ef9b1dce` "fix(cli): stop loading `.opencode` config directories" — matches the doc's migration warning (see §2) | commit history |
| 2026-07-30 | Doc commits continue (`docs(cli): deprecate Kilo Console`, "Apply suggestions from code review") | commit history |

**Read carefully:** the issue's closure is *not* evidence the feature was rejected — it reads
as a stale-bot sweep on an issue that had, in substance, already been largely addressed by
the shipped (and still-evolving) Plugins doc. Treat "closed, NOT_PLANNED" as inconclusive
process noise, not a vendor decision, per the evidence rules ("if something is undocumented,
write NOT DOCUMENTED — never guess"): whether the issue's specific asks (`kilo plugin list`,
`kilo debug plugins`) were implemented is only partially confirmed — `kilo plugin <name>`
(install) exists (§2), but a `list`/`doctor` diagnostic command is **NOT DOCUMENTED**.

---

## 2. Config location(s)

**Three loading mechanisms**, per the Plugins doc (raw source,
`packages/kilo-docs/pages/automate/extending/plugins.md`):

### (a) Config file array

```json
{
  "$schema": "https://app.kilo.ai/config.json",
  "plugin": [
    "@your-org/your-plugin",
    "your-plugin@1.2.3",
    ["your-plugin", { "apiKey": "{env:MY_API_KEY}" }],
    "./plugins/local.ts",
    "file:///abs/path/plugin.ts"
  ]
}
```

Config file itself lives at (from the CLI doc, "Config File Location (Kilo CLI 1.0)"):

| Scope | Path |
|---|---|
| Global | `~/.config/kilo/kilo.json[c]` or legacy `opencode.json[c]` |
| Project | `./kilo.json[c]`, legacy `./opencode.json[c]`, or config inside `./.kilo/` (legacy `./.kilocode/` also read) |

Format: **JSON or JSONC** (`.json` / `.jsonc`, comments preserved on programmatic edits).

Explicit precedence quote: *"Project-level configuration takes precedence over global
settings."*

Explicit **breaking-change / migration** quote (callout, CLI doc):

> **Migrating from opencode?** Kilo no longer falls back to opencode configuration stored in
> `.opencode` directories (such as `~/.config/opencode` or a project `./.opencode/`). To keep
> using it, move your global config into `~/.config/kilo/` and any project config into
> `./.kilo/`.

### (b) Auto-discovered plugin directory (no config entry needed)

> - Global: `~/.config/kilo/plugin/`
> - Project: `.kilo/plugin/` or legacy `.kilocode/plugin/`
>
> Every `.ts` or `.js` file in those directories is auto-registered at startup — no need to
> list them in the config file.

Example layout from the doc:

```text
my-project/
├── kilo.json
└── .kilo/
    └── plugin/
        ├── env-guard.ts
        └── notifications.ts
```

### (c) `kilo plugin` CLI command (installs + patches config in one step)

```bash
kilo plugin my-plugin              # project config
kilo plugin my-plugin --global     # global config
kilo plugin my-plugin --force      # replace existing entry
```

Doc quote — **note the exact filenames this command targets, which is inconsistent with
the "Config File Location" table above** (I'm flagging this verbatim rather than papering
over it, per the evidence rules):

> The command resolves the package, reads its `package.json` for plugin entrypoints, and
> writes the entry into the appropriate config file (`.kilo/opencode.jsonc` /
> `.kilo/tui.jsonc` for local installs, or `~/.config/kilo/opencode.jsonc` /
> `~/.config/kilo/tui.jsonc` for `--global`) while preserving JSONC comments.

So: the "Config File Location" table says the primary file is `kilo.json[c]`, but the
`kilo plugin` command's own doc says it writes to `opencode.jsonc`. Both are literal
quotes from the same doc page family (fetched same day). This is either (1) a genuine dual
read/write path Kilo hasn't fully renamed, or (2) a documentation inconsistency inherited
from the OpenCode fork. **NOT DOCUMENTED which is authoritative** — worth a live CLI test
before grim depends on either filename.

### Env vars

- `KILO_PURE=1` — skip all external plugins, only built-ins load ("Useful for reproducible
  CI runs or debugging").
- `KILO_CONFIG` / `KILO_CONFIG_CONTENT` — pass/relocate config content; also named as one of
  the three "trusted config" sources for `{env:VAR}` interpolation (see below).
- `XDG_CACHE_HOME` — relocates the plugin package cache: default
  `~/.cache/opencode/packages/` (note: literally "opencode" in the path, not "kilo"), override
  `$XDG_CACHE_HOME/opencode/packages/`.
- Trust boundary quote (CLI doc, warning callout) — directly relevant to third-party
  installers writing config that references secrets:
  > `{env:VAR}` (and `{file:...}`) references are resolved **only** in trusted config: your
  > global config (`~/.config/kilo`), a config passed via `KILO_CONFIG` /
  > `KILO_CONFIG_CONTENT`, or organization/MDM-managed config. A project-level `kilo.json` /
  > `opencode.json` committed to a repository **cannot** use `{env:VAR}` — the reference is
  > ignored and a warning is logged.

### Merge vs. win

**Merge, with defined precedence**, not last-source-wins-only. Verbatim "Load order" (Plugins
doc):

> Plugins from all sources run on every session. They load in this order:
> 1. Internal built-ins (Kilo Gateway auth, Codex auth, Copilot auth, Cloudflare, etc.)
> 2. Global config plugin array (`~/.config/kilo/kilo.json`)
> 3. Global plugin directory (`~/.config/kilo/plugin/`)
> 4. Project config plugin array (`kilo.json` / `opencode.json`)
> 5. Project plugin directory (`.kilo/plugin/` and friends)
>
> Duplicates (same package, same version) are deduplicated. Hooks from multiple plugins run
> sequentially in load order.

i.e. every plugin from every source is loaded (union), not one scope overriding another —
scope precedence only matters for *dedup* (same package+version collision) and for the
*order* hooks fire in, not for exclusion.

---

## 3. Config schema — verbatim

**Not a named map of `{event: handler}`.** The config-level unit is an **array of plugin
specifiers** (each specifier names a whole plugin module, not a single hook):

```ts
export type Config = Omit<SDKConfig, "plugin"> & {
  plugin?: Array<string | [string, PluginOptions]>
}
```
(`packages/plugin/src/index.ts`, raw, fetched 2026-08-14)

Each array entry is a string (npm package name, optionally `@version`, or a local
`./path.ts` / `file:///abs/path.ts`) or a `[name, options]` tuple. There is **no per-hook
config entry** — hooks only exist as properties of the `Hooks` object a plugin module
returns at runtime; you cannot declare "just the `tool.execute.before` hook" in config
without shipping/pointing to a whole plugin module.

**Identity for a third-party installer.** The closest thing to a stable id:

```ts
export type PluginModule = {
  id?: string
  server: Plugin
  tui?: never
}
```

Doc quote: *"`id` is required for local-file plugins and inferred from `package.json#name`
for npm plugins."* So a grim-authored local plugin file must self-declare an `id`:

```ts
export default { id: "hello", server: hello }
```

In practice, for a directory-auto-discovered local plugin, the **filename itself** (e.g.
`.kilo/plugin/grim-managed.ts`) is arguably the more robust ownership key for an external
installer (idempotent create/update/delete by path), with the in-module `id` as a secondary,
Kilo-visible label. Docs do not state that `id` participates in the "same package, same
version" dedup logic for local files — that dedup wording appears scoped to npm entries.
**NOT DOCUMENTED** whether two local files with the same `id` collide or coexist.

**Matcher/filter syntax:** hooks are not declaratively filtered by a glob/regex in config;
filtering happens **inside the hook function body** by inspecting the `input` object (e.g.
`if (input.tool === "bash") { ... }`, `if (event.type === "session.idle") { ... }`). This is
a materially different model from a JSON-declared `matcher` field — grim cannot express "run
only on tool X" as data; it must be baked into the generated shim's code.

A directly analogous, but separate, declarative-matcher system exists for the **built-in
permission system** (not a hook, disambiguating per brief scope): `permission.bash` accepts
glob-style rules like `"git *": "allow"`, `"rm *": "deny"`, evaluated last-match-wins. This is
config-driven policy, not a code hook, and is a different mechanism from `permission.ask`
(the plugin hook, which runs arbitrary code and returns a status).

---

## 4. Event catalogue

Two layers: (1) a fixed set of **named hooks** a plugin can implement (below), and (2) a
**generic `event` hook** that receives every message on Kilo's internal event bus, with
`event.type` distinguishing them.

### Named hooks (verbatim from `packages/plugin/src/index.ts`, the `Hooks` interface)

| Group | Hook | Fires |
|---|---|---|
| Lifecycle | `config` | Startup, once, with fully-resolved config (read-only) |
| Lifecycle | `event` | Every event on the internal bus |
| Lifecycle | `dispose` | Plugin teardown |
| Tools | `tool` | Registers a map of custom tool definitions (not a lifecycle hook — tool registration) |
| Tools | `tool.execute.before` | Before a tool call runs |
| Tools | `tool.execute.after` | After a tool call returns |
| Tools | `tool.definition` | Before a tool's description/parameters are sent to the model |
| Chat | `chat.message` | New user message received |
| Chat | `chat.params` | Before LLM call — mutate temperature/topP/topK/maxOutputTokens/options |
| Chat | `chat.headers` | Before LLM call — mutate HTTP headers |
| Chat | `permission.ask` | A permission prompt is about to be shown |
| Chat | `command.execute.before` | A slash command is about to execute |
| Chat | `shell.env` | Before any shell command the agent (or user) runs |
| Providers | `auth` | Registers an OAuth/API-key auth flow for a provider |
| Providers | `provider` | Supplies/refreshes a dynamic model catalog |
| Experimental | `experimental.chat.messages.transform` | Before full message history is sent to the model |
| Experimental | `experimental.chat.system.transform` | Before the system prompt array is finalized |
| Experimental | `experimental.provider.small_model` | Selecting the "small model" for a provider |
| Experimental | `experimental.session.compacting` | Session compaction starts (inject/replace the compaction prompt) |
| Experimental | `experimental.compaction.autocontinue` | After compaction, deciding whether to auto-send a "continue" turn |
| Experimental | `experimental.text.complete` | Post-process final text parts |

### Generic bus events (`event` hook's `event.type`, doc's own qualifier: **"Common event
types include"** — the doc explicitly does not claim this list is exhaustive)

- Session: `session.created`, `session.updated`, `session.idle`, `session.error`,
  `session.deleted`, `session.compacted`, `session.diff`, `session.status`
- Message: `message.updated`, `message.removed`, `message.part.updated`,
  `message.part.removed`
- Tool: `tool.execute.before`, `tool.execute.after` (same names as the typed hooks, also
  surfaced as bus events)
- Permission: `permission.asked`, `permission.replied`
- File: `file.edited`, `file.watcher.updated`
- Shell: `shell.env`
- Command: `command.executed`
- LSP: `lsp.updated`, `lsp.client.diagnostics`
- Todo: `todo.updated`
- Server: `server.connected`
- Installation: `installation.updated`

No dedicated "error" or "notification" event group beyond `session.error` and the CLI's
separate, non-hook "attention" notification/sound system (`tui.jsonc` `attention.*` — desktop
notification + sound on session completion/error/question; **not** hookable via a shell
command, just an enable/volume/sound-file config block. Disambiguating: this is a
notify-only feature, not an event hook, and cannot run arbitrary user code).

---

## 5. Invocation

**In-process JS/TS function call — not a subprocess.** A plugin is:

```ts
export type Plugin = (input: PluginInput, options?: PluginOptions) => Promise<Hooks>
```

Kilo's Bun/Node runtime `import()`s the module directly (from npm cache, a relative path, or
a `file://` URL) and calls the exported `server` function once per session to obtain a
`Hooks` object; each hook is then just a property on that object, invoked as a normal async
function call by the host process whenever the corresponding lifecycle point is reached.
There is **no shell command string, no argv, no separate process, and no working-directory /
`$PATH` question** for the hook invocation itself — the hook runs inside Kilo's own process
with whatever `cwd`/env the host process has. (A hook body *can* itself shell out, via the
`$` field of `PluginInput`, documented as *"Bun's shell API"* — but that's the plugin
author's choice, not part of the hook contract.)

**Ordering:** "Hooks from multiple plugins run sequentially in load order" (quoted in §2).
**NOT DOCUMENTED:** whether one hook throwing prevents subsequent plugins' same-named hook
from running (see §7 — the one worked example of blocking behavior uses a thrown
`Error`, and it's unclear if that aborts the whole chain or just that plugin's contribution).
**NOT DOCUMENTED:** any concurrency model beyond "sequential"; no explicit statement that
hooks for *different* sessions run in parallel (plausible, since Kilo supports concurrent
sessions/subagents, but not stated for hooks specifically).
**NOT DOCUMENTED:** any hook-specific timeout, default or configurable. (The *bash tool*
used by the agent has a documented 2-minute default timeout — but that is a tool, not a
hook, and the two are not stated to share a timeout budget.)

---

## 6. Input payload — verbatim

Every hook receives **two positional JS objects**, `(input, output)` — not JSON on stdin,
not env vars, not template strings. `input` is read-only context; `output` is a **mutable
object the hook edits in place** (no return value carries the result). Exact shapes
(`packages/plugin/src/index.ts`):

```ts
"permission.ask"?: (input: Permission, output: { status: "ask" | "deny" | "allow" }) => Promise<void>

"tool.execute.before"?: (
  input: { tool: string; sessionID: string; callID: string },
  output: { args: any },
) => Promise<void>

"tool.execute.after"?: (
  input: { tool: string; sessionID: string; callID: string; args: any },
  output: { title: string; output: string; metadata: any },
) => Promise<void>

"command.execute.before"?: (
  input: { command: string; sessionID: string; arguments: string },
  output: { parts: Part[] },
) => Promise<void>

"shell.env"?: (
  input: { cwd: string; sessionID?: string; callID?: string },
  output: { env: Record<string, string> },
) => Promise<void>

"chat.message"?: (
  input: { sessionID: string; agent?: string; model?: {providerID:string; modelID:string}; messageID?: string; variant?: string },
  output: { message: UserMessage; parts: Part[] },
) => Promise<void>

"chat.params"?: (
  input: { sessionID: string; agent: string; model: Model; provider: ProviderContext; message: UserMessage },
  output: { temperature: number; topP: number; topK: number; maxOutputTokens: number | undefined; options: Record<string, any> },
) => Promise<void>

event?: (input: { event: Event }) => Promise<void>
config?: (input: Config) => Promise<void>
```

Real, doc-provided worked examples (verbatim):

```ts
// .kilo/plugin/env-guard.ts
const EnvGuard: Plugin = async () => ({
  "tool.execute.before": async (input, output) => {
    if (input.tool === "read" && String(output.args.filePath).includes(".env")) {
      throw new Error("reading .env files is blocked")
    }
  },
})
export default { id: "env-guard", server: EnvGuard }
```

```ts
// .kilo/plugin/inject-env.ts
const InjectEnv: Plugin = async () => ({
  "shell.env": async (input, output) => {
    output.env.MY_API_KEY = "secret"
    output.env.PROJECT_ROOT = input.cwd
  },
})
export default { id: "inject-env", server: InjectEnv }
```

Custom-tool `execute(args, context)` gets `context = { sessionID, messageID, agent,
directory, worktree, abort, metadata, ask }` (doc, verbatim list).

No stdin/env-var/argv/template-string channel exists for hook payload delivery — this is
purely a typed in-process function call contract.

---

## 7. Output / response contract — verbatim

**No exit codes, no parsed stdout/stderr — because there is no subprocess.** The response
contract is: **mutate the `output` object's fields, or throw.**

- `permission.ask`: set `output.status` to exactly one of the literal union
  `"ask" | "deny" | "allow"` (same three-value vocabulary the static `permission` config
  block uses — see §3).
- `tool.execute.before` / `command.execute.before`: mutate `output.args` /
  `output.parts` to change what actually runs.
- **Blocking/denying a tool call is done by throwing a JS `Error`**, not by setting a
  status field — see the `.env`-guard example above (`throw new Error(...)`). This is a
  materially different denial mechanism from `permission.ask`'s `output.status = "deny"`.
  **NOT DOCUMENTED**: what user-facing message/UI results from a thrown error (is the
  error string shown to the user, the model, both?) — the doc doesn't say beyond implying
  the tool call is stopped.
- `tool.execute.after`: mutate `output.title` / `output.output` / `output.metadata` to
  rewrite what the model and UI see as the tool's result.
- `chat.params` / `chat.headers` / `shell.env`: mutate the respective `output` fields
  (numbers, header map, env map) directly.
- `experimental.session.compacting`: push strings onto `output.context`, or set
  `output.prompt` to fully replace the compaction prompt (doc: *"Set `output.prompt` to
  replace the default compaction prompt entirely — when present, `output.context` is
  ignored."*).
- `experimental.compaction.autocontinue`: set `output.enabled` (boolean, defaults to
  `true`) to suppress the synthetic "continue" turn.
- `event`/`config`: **read-only observers** — no `output` parameter at all, return value is
  `void`. Cannot influence anything; purely for side effects (logging, external calls).

**stdout/stderr:** not part of the contract at all for the hook function itself (it's not a
process). The doc recommends `client.app.log()` over `console.log` specifically so log
lines land in Kilo's own log pipeline (levels: `debug`, `info`, `warn`, `error`) rather than
wherever raw stdout would otherwise go — implying raw `console.log` output from a plugin is
*not* guaranteed to be surfaced anywhere useful to the user.

**Malformed output:** **NOT DOCUMENTED.** No spec for what happens if `output.status` is set
to a string outside the 3-value union, or if a hook returns before mutating required fields.
TypeScript typing is the only stated guard, which is a compile-time, not runtime, contract —
irrelevant to a plain-JS or misbehaving plugin at runtime.

---

## 8. Reliability & limits

- **Timeout:** NOT DOCUMENTED for hook execution. (Bash *tool* calls default to 2 minutes,
  overridable per-call via the `workdir`/timeout tool args — but that's the agent's bash
  tool, not a plugin hook.)
- **Non-zero "exit":** N/A framing — there's no process/exit code. The closest analog,
  a thrown exception in a hook, is only documented to behave a specific way for
  `tool.execute.before` (blocks that tool call). Behavior for an uncaught throw in
  `chat.params`, `event`, etc. is **NOT DOCUMENTED** — could plausibly crash the session,
  be swallowed, or log-and-continue; the doc doesn't say.
- **Missing binary:** N/A (no external binary is invoked by the contract itself). A
  *missing npm plugin package* fails at resolve time; the doc's troubleshooting section says
  load failures are "surfaced as session errors in the TUI and VS Code extension" and
  visible via `kilo --print-logs --log-level DEBUG`.
- **Parallel vs sequential:** documented as **sequential**, in a fixed load order (§2/§5).
- **Blocking vs fire-and-forget:** all hooks are `async` functions the host `await`s (return
  type `Promise<void>` throughout) — the host necessarily blocks on each hook before
  proceeding with the lifecycle step it gates (e.g. `tool.execute.before` must resolve
  before the tool actually executes). Fire-and-forget is not offered as an option.
- **Config snapshot / restart gotcha (ties into Q10):** plugins are explicitly *"TypeScript
  or JavaScript modules loaded at startup"* (doc, opening paragraph). The CLI doc separately
  instructs: *"Restart the CLI after editing"* `kilo.jsonc`. The `/reload` slash command is
  documented to *"Reload config, skills, agents, and commands from disk"* — **plugins are
  conspicuously absent from that list**, so a newly dropped/edited plugin file most likely
  needs a full process restart (new `kilo` invocation / new VS Code session), not just
  `/reload`. **NOT DOCUMENTED explicitly either way** — flagging the omission rather than
  assuming.

---

## 9. Security posture

**No dedicated "hooks/plugins are arbitrary code, here's our trust model" page was found.**
I checked the `/docs/deploy-secure` section (which turned out to be about app deployment +
Dependabot-alert triage, unrelated to plugin trust) and grepped the whole `kilo-docs`
sources for "trust"; the only relevant hit is in the **MCP** FAQ (a disambiguated, adjacent
mechanism, not a hook, but the vendor's closest stated general policy toward user-supplied
extensions):

> **How is security handled?** Users control which MCP servers they connect to and what
> permissions those servers have. As with any tool that accesses data or services, use
> trusted sources and configure appropriate access controls.
> (`packages/kilo-docs/pages/automate/mcp/what-is-mcp.md`)

For plugins specifically, I found **no** documented:
- prompt/confirmation before a newly-discovered `.kilo/plugin/*.ts` file executes,
- allowlist or signature/trust mechanism for plugin sources,
- warning banner comparable to, say, a "this repo can run code on your machine" notice.

Local plugin files are auto-registered "at startup — no need to list them in the config
file" (§2), and a `package.json` dropped next to them triggers an unprompted `bun install`
at startup (§2/§8's troubleshooting section: *"Kilo runs `bun install` on startup so your
plugins can import the packages"*). Combined, this means **cloning a repo with a
`.kilo/plugin/anything.ts` file and opening it in Kilo — CLI or VS Code — executes that
file's top-level code the next time a session starts, with no visible gate**, beyond the
blanket `KILO_PURE=1` env var to opt out of all external plugins entirely. This is a real
gap worth flagging to whoever designs grim's hook trust model, independent of grim's own
behavior.

The one *quasi*-mitigation documented: **npm-plugin install scripts are blocked** —
*"Install scripts are disabled for npm plugins. Kilo installs packages with lifecycle
scripts such as `install` and `postinstall` blocked."* This only covers the npm-install
supply-chain vector, not the plugin's own module code (which runs unconditionally on
import) nor local `.ts`/`.js` files dropped directly into a plugin directory.

---

## 10. Third-party installability

**Yes, realistically, by file drop alone — no vendor CLI required** for the directory
mechanism:

- Writing a `.ts`/`.js` file into `.kilo/plugin/` (project) or `~/.config/kilo/plugin/`
  (global) is sufficient; Kilo auto-registers it "at startup — no need to list them in the
  config file."
- Alternatively, splicing a `"plugin": [...]` array entry into `kilo.json[c]` (JSON/JSONC —
  grim already has to preserve-byte-splice JSON/TOML for other clients per its own
  architecture, so this is the same class of operation) achieves the same registration
  without touching a directory.
- The vendor's own `kilo plugin <name>` command is offered as a *convenience*, not a
  requirement — it "resolves the package... and writes the entry into the appropriate
  config file... while preserving JSONC comments," i.e. it does exactly what grim's own
  splice-in-place approach already does, just for npm-hosted plugins specifically. Grim does
  not need to shell out to `kilo` to install a hook.

**Restart-needed gotcha:** confirmed, see §8 — "loaded at startup," `/reload`'s documented
scope excludes plugins, and the CLI doc's own instruction ("Restart the CLI after editing")
for the adjacent `kilo.jsonc` settings. A grim-installed hook plugin will not take effect in
an already-running `kilo` TUI session or an already-open VS Code window; the user (or CI job)
needs a fresh session.

---

## 11. Trampoline viability

**Not viable as a single generic native command.** This is the load-bearing finding for the
portable-hook-schema design. Contrast with a client whose hook contract is already
"spawn shell command, JSON on stdin, exit code + stdout JSON back" (trivially wrappable as
`grim hook run --client X --event E`): Kilo/OpenCode's contract is **in-process JS/TS
function call**, with:

1. **No shell-command hook shape exists at all.** A plugin can *only* be a JS/TS module that
   Kilo's Bun/Node runtime `import()`s. There is no config shape anywhere (array entry,
   directory file, or otherwise) that means "run this shell command when event E fires."
2. **No stdin/argv/env payload channel.** Payload delivery is two JS objects passed as
   function arguments (`input`, `output`), not serialized data a subprocess could read.
3. **Per-hook-type response shapes differ structurally** (`output.status` enum for
   `permission.ask`; `output.args` mutation for `tool.execute.before`; a *thrown exception*
   for denial in the same hook; `output.env` map for `shell.env`; void/no-output for
   `event`/`config`). A generic trampoline can't have one universal "the subprocess exited 2,
   therefore deny" rule — it needs per-hook-type translation logic baked into whatever shim
   code runs inside the JS function.
4. **Consequence:** the only workable design is a **generated, per-hook-type JS/TS shim
   file** that grim writes into `.kilo/plugin/` (e.g. `.kilo/plugin/grim-hooks.ts`), where
   the shim's own code (not a config value) does the bridging: spawn `grim hook run
   --client kilo --event <E>` as a child process (via the `$` Bun-shell field already handed
   to the plugin, or plain `child_process`), serialize `input`/relevant `output` fields to
   JSON on that subprocess's stdin, parse its stdout as JSON, and then — depending on which
   hook this shim function is for — either mutate `output.*` fields or `throw` to deny. This
   is fully buildable (grim already generates/splices files for other clients) but it is
   **a per-hook-code-generation problem, not a single generic command line**, and every time
   Kilo/OpenCode adds or reshapes a hook signature, the shim template needs a matching
   update. Given `packages/plugin/src/index.ts` is mechanically synced from upstream OpenCode
   releases every few weeks (§1 timeline), this template would need active maintenance.
5. Minor secondary blockers: engine-compat gating uses `"engines": { "opencode": "^X.Y.Z" }`
   (literal key `"opencode"`, confirmed still true in current docs/source — §0's rebrand
   residue) — grim's generated shim's `package.json` (if it ships one for deps) would need
   to declare compatibility against upstream OpenCode version numbers, not a Kilo-specific
   scheme, to avoid being silently skipped ("If the running CLI does not satisfy the range,
   the plugin is skipped and a warning is surfaced").

**Bottom line for Q11:** buildable, not trivial. Requires shipping/maintaining TypeScript
shim code per hook-type (not just a schema mapping), plus a JSON-over-stdin sub-protocol
grim invents itself (Kilo has no native stdin/JSON hook contract to reuse).

---

## Disambiguation (adjacent, NOT hooks, per brief scope)

- **MCP servers** (`/docs/automate/mcp/*`) — separate protocol-based tool integration,
  user/vendor-controlled trust ("use trusted sources"), not user-code-at-lifecycle-event.
- **Custom tools** (`tool: { ... }` in a plugin, or standalone files in `.kilo/tool/` /
  `~/.config/kilo/tool/`) — model-invokable capabilities, not lifecycle hooks (though they
  are *defined* inside the same plugin module type).
- **Shell/bash tool** (`/docs/automate/extending/shell-integration`) — the tool the *agent*
  uses to run commands (Tree-sitter-based command analysis, 2-minute default timeout,
  `workdir` param). Not a hook; it's the thing hooks like `tool.execute.before` can intercept.
- **Custom modes / agents** (Architect, Ask, Debug, Orchestrator, custom agent modes) —
  prompt/persona configuration, not event-driven code execution.
- **CLI notifications/sounds** (`tui.jsonc` → `attention.*`) — config-only enable/sound-file
  system, explicitly documented as *not* needing a plugin: *"You do not need a plugin or
  platform-specific notification command."* No arbitrary code runs.
- **`.kilocode/` (legacy)** — still read for both config and plugin auto-discovery
  (confirmed in §2/§0), not merely a detection-only artifact — correcting a possible
  assumption in the brief.
- **Git hooks** — out of scope per the brief; not mentioned anywhere in Kilo's docs as a
  Kilo-specific feature.

---

## Sources

| URL | What it establishes | Fetched |
|---|---|---|
| https://kilocode.ai/docs → redirects to https://kilo.ai/docs | Confirms the kilocode.ai → kilo.ai domain migration (308 permanent redirect); top-level doc site structure | 2026-08-14 |
| https://kilo.ai/docs/automate | "Automate" section structure, links to Plugins/MCP/Agent Manager pages | 2026-08-14 |
| https://kilo.ai/docs/automate/extending/plugins (rendered) | First-pass confirmation of plugin/hook system, config array, directories, hooks list | 2026-08-14 |
| https://github.com/Kilo-Org/kilocode raw: `packages/kilo-docs/pages/automate/extending/plugins.md` | **Verbatim** Plugins/Hooks doc source — all quotes in §1–§9 not otherwise attributed | 2026-08-14 |
| https://github.com/Kilo-Org/kilocode raw: `packages/plugin/src/index.ts` | **Verbatim** TypeScript `Hooks`/`Plugin`/`PluginInput`/`Config`/`PluginModule` type definitions — ground truth for §3, §6, §7 | 2026-08-14 |
| https://github.com/Kilo-Org/kilocode raw: `packages/kilo-docs/pages/code-with-ai/platforms/cli.md` | Verbatim CLI doc: config file locations, permission model (`allow`/`ask`/`deny`), env var overrides, exit codes (`0`/`1`/`124`), `.opencode`-fallback removal notice, `/reload` scope | 2026-08-14 |
| https://kilo.ai/docs/automate/extending/shell-integration | Bash-tool execution model (disambiguation only, not a hook) | 2026-08-14 |
| https://github.com/Kilo-Org/kilocode/issues/9476 (`gh issue view --json`) | Exact issue body, author (`alphaDev23`), created `2026-04-24T17:20:03Z`, closed `2026-06-24T07:06:48Z`, `stateReason: NOT_PLANNED`, sole comment is the stale-bot auto-close message | 2026-08-14 |
| https://blog.kilo.ai/p/new-kilo-for-vs-code-is-live (author: Job Rietbergen) | GA date 2026-04-02, "completely rebuilt" on OpenCode server, shared engine across VS Code/CLI/Cloud Agents, JetBrains/CLI/Cloud Agents platform list | 2026-08-14 |
| `gh api repos/Kilo-Org/kilocode/commits?path=packages/kilo-docs/pages/automate/extending/plugins.md` | Commit-dated timeline of the Plugins doc (first found commit 2026-04-27 "clarify plugin cache path"; 11 pages of history) | 2026-08-14 |
| `gh api repos/Kilo-Org/kilocode/commits?path=packages/plugin/src/index.ts` | Commit messages "OpenCode v1.16.2 (#12088)" (2026-07-13), "refactor: kilo compat for v1.15.13" (2026-06-18), "for v1.14.51" (2026-06-09) — proves mechanical upstream-OpenCode sync of the plugin type package | 2026-08-14 |
| https://github.com/Kilo-Org/kilocode raw: `packages/kilo-docs/pages/deploy-secure/index.md` | Confirms "Deploy & Secure" section is about app deploy + Dependabot triage, **not** plugin/hook security — used as absence-evidence for §9 | 2026-08-14 |
| https://github.com/Kilo-Org/kilocode raw: `packages/kilo-docs/pages/automate/mcp/what-is-mcp.md` (grepped) | MCP security FAQ quote ("use trusted sources...") — closest documented vendor security stance, explicitly MCP-scoped not plugin-scoped | 2026-08-14 |
| WebSearch: "Kilo Code opencode plugin merger CLI 2026" | Corroborating context (Respan comparison article, explainx.ai blog) that Kilo CLI is "a fork of OpenCode" — triangulation only, primary facts drawn from Kilo's own docs/source above | 2026-08-14 |
