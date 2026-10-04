# Goose (Block → Agentic AI Foundation) — native hook mechanism

Researched 2026-08-14. Client: **goose**, CLI/desktop AI agent.

## ⚠️ Ownership/domain migration — update the brief's "known paths"

The brief's starting facts (`block/goose`, `https://block.github.io/goose/`) are **stale**.
Verified from three independent primary sources:

- `https://block.github.io/goose/` now serves a banner: **"🦆 goose has moved!"** with
  "Redirecting to [goose-docs.ai](https://goose-docs.ai)…" (fetched 2026-08-14).
- `https://goose-docs.ai/blog/2026/04/07/goose-moves-to-aaif/` (fetched 2026-08-14, verbatim):
  "Block has donated goose to the Agentic AI Foundation (AAIF) at the Linux Foundation,
  alongside Anthropic's Model Context Protocol (MCP)" — transition dated **2026-04-07**. "The
  old docs links will redirect, but update your bookmarks to the new site." GitHub org moved
  from `block/goose` to `https://github.com/aaif-goose/goose`.
- `https://api.github.com/repos/block/goose` (GitHub's own API, fetched 2026-08-14) resolves
  to `"full_name": "aaif-goose/goose"`, `"homepage": "https://goose-docs.ai/"`,
  `"archived": false`, 52,789 stars, `pushed_at: 2026-08-14` (actively maintained).
- Independently corroborated by the Linux Foundation's own press release ("Linux Foundation
  Announces the Formation of the Agentic AI Foundation (AAIF), Anchored by New Project
  Contributions Including Model Context Protocol (MCP), goose and AGENTS.md",
  linuxfoundation.org, and mirrored on prnewswire.com and aaif.io — foundation announced
  December 2025).

**Everything below uses the current canonical sources**: repo `github.com/aaif-goose/goose`,
docs `goose-docs.ai`. The Windows config path still literally contains `Block\goose` (legacy
branding kept for compatibility) — see §2.

A word of caution on method: search-engine snippets for this client are heavily polluted with
non-official mirrors and SEO/wiki farms — `block-goose.mintlify.app`, `instagit.com`,
`contextqmd.com`, `awesome-goose.github.io`, `agent-safehouse.dev`, `deepwiki.com`, and at
least two full source forks (`leanzero-srl/goose-local-edition`,
`ai-skynet-labs/ai-coding-goose`) all rank for goose config queries. None of those are cited
below as fact; every claim here is traced to `goose-docs.ai`, `github.com/aaif-goose/goose`
(including raw file contents and a merged PR), or the AAIF/Linux Foundation press materials.

---

## 1. Existence & name

**Exists. Vendor calls it "hooks."** Confirmed via the official announcement blog post
(`goose-docs.ai/blog/2026/05/14/goose-hooks/`, fetched 2026-08-14):

> "goose now supports **lifecycle hooks**. Drop a plugin into a directory on disk and goose
> will run your shell scripts when things happen during a session: a tool is about to fire, a
> tool just finished, the user submitted a prompt, the session started, the session ended...
> If you've used Claude Code's hooks or git hooks, it's the same idea, and the agent loop is
> now scriptable from the outside, without writing any Rust or any MCP server."

- **Delivery vehicle**: hooks ship as one part of a **plugin** (Goose's packaging unit, which
  also carries Skills). Hooks are not a config.yaml feature; they are a plugin-directory
  feature (see §2).
- **Version introduced**: GitHub Release `v1.34.0` (2026-05-13, `github.com/aaif-goose/goose/releases/tag/v1.34.0`, fetched 2026-08-14), which lists "Hooks support for customizable
  agent behavior" (PR #9093) and "Install plugins to ~/.agents/plugins" (PR #9088) among that
  release's features.
- **Refined in**: PR **#9304**, `feat(hooks): PreToolUse denial`, merged ~2026-05-19
  (`github.com/aaif-goose/goose/pull/9304`, fetched 2026-08-14) — added the deny/block
  semantics described in §7. Search results (not independently re-fetched, so treated as
  **[unofficial]** for exact wording) place further "open-plugins generalization plus skills"
  work in v1.35.0 (2026-05-22) and v1.36.0 (2026-05-27).
- **Stability**: no `beta`/`experimental`/feature-flag label found on the hooks feature itself
  in any fetched source. It ships in mainline releases and is documented on the permanent docs
  site (not just a blog post) at `goose-docs.ai/docs/guides/context-engineering/hooks`. No
  deprecation notice found.
- **Naming caution — "Open Plugins" spec**: Goose's docs repeatedly say hooks "follow the Open
  Plugins hooks specification" / are discovered "per the Open Plugins installation
  specification." I could **not** find any specification document, org, or URL for "Open
  Plugins" independent of Goose's own docs — every hit traces back to `goose-docs.ai` itself.
  This is a **different thing** from the real, independently-launched **Agent Plugins**
  standard (`agent-plugins.org`, `github.com/agentplugins/agent-plugins-spec`, v1.0.0, backed
  by OpenAI/AWS/Cursor/GitHub/Microsoft/Vercel, announced 2026-08-06). Fetched directly
  (`agent-plugins.org`, 2026-08-14): that spec explicitly states hooks are **not** a portable
  v1 component — "Reverse-domain extension namespaces let individual clients add behavior
  without changing the portable core," with example hook placement under
  `com.example.client/hooks/`. Its Technical Steering Committee is "Core Maintainers from
  Amazon, Cursor, Microsoft, OpenAI, and Vercel" — Goose/AAIF is not named as a founding
  adopter in what I fetched. **Conclusion: treat "Open Plugins" as Goose's own name for its
  own plugin/hook format, not a ratified cross-vendor standard.** This matters directly for
  grim's trampoline design — there is no external body enforcing this shape; Goose chose it
  unilaterally. Consistent with that reading, PR #9304 itself says the denial protocol
  followed "precedent for adopting **Claude Code's hook conventions**," while "field-naming
  alignment for the broader `HookContext` payload remained unresolved" — i.e., Goose's own
  engineers describe this as deliberately copying Claude Code, not implementing an external
  spec.

## 2. Config location(s)

Two **separate** file families are involved, and hooks live in neither of the client's main
settings files:

| Purpose | Path (project) | Path (global/user) | Format |
|---|---|---|---|
| Provider/model/MCP-extension settings | none (project scope not documented for this file) | macOS/Linux: `~/.config/goose/config.yaml`; Windows: `%APPDATA%\Block\goose\config\config.yaml` | **YAML** |
| Plugin disable list | — | `~/.config/goose/settings.json` (key `disabledPlugins`) | **JSON** |
| **Hooks (+ Skills) — the actual hook config** | `<project-root>/.agents/plugins/<plugin-name>/hooks/hooks.json` | `~/.agents/plugins/<plugin-name>/hooks/hooks.json` | **JSON** |

Sources:
- `goose-docs.ai/docs/guides/config-files/` (fetched 2026-08-14): gives the config.yaml paths
  above and enumerates every top-level key the file supports — `active_provider`,
  `providers`, `extensions`, plus ~25 `GOOSE_*`/`SECURITY_PROMPT_*`/`otel_*` scalar settings
  and `slash_commands`. Verbatim: **"'Hooks,' 'plugins,' and 'disabledPlugins' do not appear
  anywhere on this page."** The page also does not mention `GOOSE_PATH_ROOT` or
  `XDG_CONFIG_HOME` at all — I could not confirm those from official docs (the brief's own
  framing asserts `$GOOSE_PATH_ROOT` replaces the config-file candidate list; I did not find
  a primary source for that during this pass, so treat it as **NOT CONFIRMED BY ME, NOT
  CONTRADICTED EITHER** — it may be sourced from grim's own code/tests rather than docs).
- `goose-docs.ai/docs/guides/context-engineering/plugins/` (fetched 2026-08-14): plugin
  discovery paths, quoted verbatim as `~/.agents/plugins/<plugin-name>/` (user) and
  `<project>/.agents/plugins/<plugin-name>/` (project) — **this is the same shared
  cross-tool `.agents` pool grim already targets for skills**, not a goose-specific XDG path.
  Release v1.34.0's own changelog line is literally "Install plugins to ~/.agents/plugins"
  (PR #9088).
- `raw.githubusercontent.com/aaif-goose/goose/main/examples/plugins/hello-hooks/README.md`
  (fetched 2026-08-14, verbatim): "To turn the plugin off, add it to `disabledPlugins` in
  `~/.config/goose/settings.json`: `{ "disabledPlugins": ["hello-hooks"] }`" — confirming
  `settings.json` (JSON, not YAML) is the file with the one hook-adjacent config.yaml-family
  touchpoint.
- Alternate manifest locations for the plugin root itself, per the Plugins guide (fetched
  2026-08-14): "Open Plugins can use `plugin.json` at the plugin root, `.plugin/plugin.json`,
  or `.goose-plugin/plugin.json`." Only `plugin.json` at plugin root was seen in the actual
  `hello-hooks` example.
- **Directory auto-discovery**: yes — any subdirectory of the two plugin roots containing a
  `hooks/hooks.json` (or a manifest) is auto-discovered; no central registration list of
  hook files exists elsewhere.
- **Merge vs. one-wins**: **NOT DOCUMENTED.** I could not find text stating what happens when
  a plugin of the same name exists at both project and user scope (the Plugins guide page, as
  summarized to me, explicitly "does not specify precedence or merge rules" for that case).

## 3. Config schema — verbatim

**It is a named map, not a flat array.** Top key `"hooks"` maps **event name → array of
matcher-groups**, each matcher-group holding an array of hook actions. Exact file, fetched
byte-for-byte from `raw.githubusercontent.com/aaif-goose/goose/main/examples/plugins/hello-hooks/hooks/hooks.json` (2026-08-14):

```json
{
  "hooks": {
    "SessionStart": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "${PLUGIN_ROOT}/scripts/announce.sh start"
          }
        ]
      }
    ],
    "UserPromptSubmit": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "${PLUGIN_ROOT}/scripts/announce.sh prompt"
          }
        ]
      }
    ],
    "PreToolUse": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "${PLUGIN_ROOT}/scripts/announce.sh pre-tool"
          }
        ]
      }
    ],
    "PostToolUse": [
      {
        "matcher": "developer__shell|developer__text_editor",
        "hooks": [
          {
            "type": "command",
            "command": "${PLUGIN_ROOT}/scripts/announce.sh post-tool"
          }
        ]
      }
    ]
  }
}
```

This shape — `{"hooks": {"<Event>": [{"matcher": <regex>, "hooks": [{"type":"command",
"command": <string>}]}]}}` — is **structurally identical** to Claude Code's own
`settings.json` hooks block (matcher-group wrapping an array of command entries), which lines
up with PR #9304's own admission of copying "Claude Code's hook conventions" (§1).

Per-hook-action fields seen: `type` (only value observed: `"command"`), `command` (string,
supports `${PLUGIN_ROOT}` interpolation — see §5), and optionally `timeout` (integer seconds),
shown in the Hooks reference page's override example (fetched 2026-08-14):

```json
{
  "type": "command",
  "command": "${PLUGIN_ROOT}/scripts/log-tool.sh",
  "timeout": 10
}
```

**Matcher syntax**: a single string field, `"matcher"`, tested as a **regex** — confirmed both
by the literal alternation pattern in the real example (`"developer__shell|developer__text_editor"`) and by a docs paraphrase describing it as "a regex tested against the most relevant
string for the event (tool name, file path, or shell command)." Omitting `matcher` fires the
hook for every event of that type (seen on `SessionStart`/`UserPromptSubmit`/`PreToolUse`
above, all `matcher`-less). No separate glob syntax was found.

**Stable identity for a third-party installer**: individual hook *entries* inside `hooks.json`
carry **no id/name/description field** — identity is not tracked at that granularity. The
**plugin as a whole** is the addressable, ownable unit: its directory name doubles as its
name, and `plugin.json` declares `name`/`version`/`description` redundantly. Exact
`plugin.json` for `hello-hooks`, fetched verbatim:

```json
{
  "name": "hello-hooks",
  "version": "0.1.0",
  "description": "A tiny demo plugin that prints messages so you can see goose's hook system firing in real time."
}
```

**Implication for grim**: a grim-installed hook artifact = one whole plugin directory (e.g.
`~/.agents/plugins/grim-<artifact-name>/` containing `plugin.json` + `hooks/hooks.json` +
any scripts) owned entirely by grim. That is a clean, idempotent install/update/remove unit —
delete the directory to remove, rewrite it to update — **so long as grim reserves the whole
directory** (no need to splice inside a shared file the way config.yaml/JSON-splicing works
for other clients). Toggling a plugin off without deleting it means writing into the
*separate* `~/.config/goose/settings.json` `disabledPlugins` array — a real JSON-splice point
if grim ever wants "disable, don't remove" semantics.

## 4. Event catalogue

Verbatim from `goose-docs.ai/docs/guides/context-engineering/hooks` (fetched 2026-08-14),
event name and firing condition as documented:

| Event | Fires when |
|---|---|
| `SessionStart` | "A session starts" |
| `SessionEnd` | "A session ends" |
| `Stop` | "goose finishes a turn or receives a stop event" |
| `UserPromptSubmit` | "The user submits a prompt" |
| `PreToolUse` | "Before goose runs a tool" |
| `PostToolUse` | "After a tool succeeds" |
| `PostToolUseFailure` | "After a tool fails" |
| `BeforeReadFile` | "Before goose reads a file" |
| `AfterFileEdit` | "After goose successfully edits a file" |
| `BeforeShellExecution` | "Before goose runs a shell command" |
| `AfterShellExecution` | "After goose successfully runs a shell command" |

Grouped against the brief's taxonomy:
- **Session lifecycle**: `SessionStart`, `SessionEnd`
- **Prompt submit**: `UserPromptSubmit`
- **Pre/post tool use**: `PreToolUse`, `PostToolUse`, `PostToolUseFailure`
- **File edit**: `BeforeReadFile`, `AfterFileEdit`
- **Command execution**: `BeforeShellExecution`, `AfterShellExecution`
- **Stop/finish**: `Stop`
- **Notification / compaction / subagent / error**: **NOT DOCUMENTED** — no event names found
  for a standalone "notify," context-compaction, subagent-spawn, or generic-error hook. (Note:
  config.yaml separately has `GOOSE_AUTO_COMPACT_THRESHOLD`, a compaction *setting*, not a
  compaction *hook event*.)

I could not determine from docs alone which exact release added the file/shell sub-events
(`BeforeReadFile`/`AfterFileEdit`/`BeforeShellExecution`/`AfterShellExecution`) versus the
original v1.34.0 set — **NOT DOCUMENTED** which point release added them; treat all as
available in current mainline.

## 5. Invocation

- **Executed as**: a shell command **string** (`"command": "..."`), not an argv array, not a
  JS/TS module import, not an HTTP call. `type` is presently always `"command"` in every
  example found — no other `type` value documented.
- **Interpolation**: `${PLUGIN_ROOT}` inside the `command` string expands to the plugin's own
  directory, letting a script reference sibling files (`${PLUGIN_ROOT}/scripts/log.sh`). No
  other interpolation tokens were found documented.
- **Working directory**: **NOT DOCUMENTED** explicitly (no page fetched states the hook
  process's cwd). The payload does carry a `working_dir` field for tool-scoped events (§6),
  which the hook script can act on itself, but that is payload data, not a stated guarantee
  about the spawned process's own cwd.
- **Shell used / `$PATH` handling**: **NOT DOCUMENTED** in any fetched source.
- **Timeout**: default **30 seconds** ("Defaults to 30 seconds," `goose-docs.ai` hooks
  reference, fetched 2026-08-14), overridable per-hook-action via the `"timeout"` integer
  field (seconds) shown in §3.
- **Ordering**: for `PreToolUse` specifically, PR #9304 states hooks "Run[] hooks in order and
  stop[] at the first explicit deny" — i.e., sequential, short-circuiting on first `block`.
  Ordering/parallelism for the non-blocking events is **NOT DOCUMENTED**.
- **A cap exists on consecutive `Stop` blocks**, raised via env var
  `GOOSE_STOP_HOOK_BLOCK_CAP` (hooks reference page, fetched 2026-08-14) — prevents a
  misbehaving `Stop` hook from looping the agent forever by repeatedly refusing to let it
  finish.

## 6. Input payload — verbatim

**Transport**: JSON on **stdin**. Quoted directly: "When a hook runs, goose writes a JSON
payload to the command's stdin." (`goose-docs.ai` hooks reference, fetched 2026-08-14.) No
env-var or argv payload delivery documented, and no `{{template}}`-style interpolation into
the command string beyond `${PLUGIN_ROOT}` (§5) was found.

Fields, as documented:
- **Always present**: `event` (event name string), `session_id`.
- **Conditionally present**: `matcher_context` (the string actually tested against
  `matcher`), `tool_name`, `tool_input` (tool events), `message` (prompt text,
  `UserPromptSubmit`), `last_assistant_message` (`Stop`, when output exists), `working_dir`
  (tool events).

Three verbatim examples from the docs:

```json
{
  "event": "PostToolUse",
  "session_id": "abc-123",
  "matcher_context": "developer__shell",
  "tool_name": "developer__shell",
  "tool_input": { "command": "rg TODO" },
  "working_dir": "/Users/you/project"
}
```

```json
{
  "event": "UserPromptSubmit",
  "session_id": "abc-123",
  "matcher_context": "summarize this file",
  "message": "summarize this file"
}
```

```json
{
  "event": "Stop",
  "session_id": "abc-123",
  "last_assistant_message": "Done. I updated the file and ran the tests."
}
```

## 7. Output / response contract — verbatim

**Only two events can block: `PreToolUse` and `Stop`.** Quoted: "Two events are
different—`PreToolUse` and `Stop` can block." Every other event is fire-and-forget/
observation-only — "Hook return value is ignored" for those.

**Deny signal (either one triggers a block), for `PreToolUse`**:
1. Process **exits with code 2** — the deny reason is taken from **stderr**.
2. Process exits 0 (or anything else) and **stdout begins with `{`** — parsed as JSON; if
   `decision` is exactly the string `"block"`, the call is denied and `reason` (from the JSON)
   is used as the reason. Quoted: "stdout must start with `{` and `decision` must be exactly
   `"block"`; any other value allows the call. If the `reason` is empty, goose substitutes
   `denied by plugin hook`."

Exact JSON shape for a denial:
```json
{"decision":"block","reason":"..."}
```

**What the model sees on denial** (quoted, reconstructed from the docs fetch): goose returns a
tool result to the model reading approximately: `Tool call denied by policy: <reason>. Do not
retry; this is a policy denial, not a transient failure.` PR #9304 separately notes an
implementation adjustment: denial is surfaced as a tool result with `is_error: true` (changed
from an `INVALID_REQUEST`-style error) specifically "preventing unintended model retry loops
on denied calls."

**Fail-open, explicitly**: quoted, "A broken hook fails open. goose blocks only on one of the
two deny signals above. If the hook produces neither — it prints nothing or non-`{` stdout and
does not exit `2`, or it fails to run at all (a spawn error or a timeout) — the call is logged
and allowed." PR #9304, independently: "Any other failure — spawn errors, timeouts, other
non-zero exits — is logged and treated as Allow, so a misbehaving hook can't block."

**No `allow`/`ask` value was documented** — only `block` is a meaningful `decision` value;
anything else (including an explicit `"allow"`, presumably) "allows the call" per the quote
above, but no permission-tier system (`allow`/`deny`/`ask` three-way, or a modified-input
passthrough, or "add context" field) was found for goose hooks, unlike Claude Code's richer
`hookSpecificOutput`/`additionalContext`/`permissionDecision` surface. Goose's contract is
strictly binary: allow, or block-with-reason.

**stderr/stdout visibility**: stderr is consumed programmatically as the deny reason on exit
code 2; beyond that, whether stderr/stdout are ever surfaced to the end user in a UI/log (as
opposed to just the model-facing denial string) is **NOT DOCUMENTED** in what I fetched.
Separately, hook failures are logged: "Hook failures are logged but do not crash goose or the
tool that triggered the hook" — but I did not confirm exactly where that log goes (file? CLI
stderr? OTel span?).

## 8. Reliability & limits

- **Timeout**: 30s default, per-action override (§5).
- **Non-zero exit** (other than `2`, for `PreToolUse`): fail-open/Allow.
- **Malformed JSON on stdout**: not explicitly named as a case, but falls under "non-`{`
  stdout" → fail-open/Allow by the general rule quoted in §7.
- **Missing binary** (spawn error): fail-open/Allow, logged.
- **Parallelism**: NOT DOCUMENTED in general; the one concrete guarantee is `PreToolUse`
  hooks run "in order" (sequential) and short-circuit on first deny (§5, PR #9304).
- **Blocking vs fire-and-forget**: `PreToolUse` and `Stop` are blocking (the agent waits for
  the decision, up to the timeout); every other event's hook runs but its result is discarded
  — whether the agent *waits* for those to finish before proceeding, or fires them
  asynchronously, is **NOT DOCUMENTED**.

## 9. Security posture

Exact quoted warning from the Hooks reference page (bolded callout in the original, fetched
2026-08-14):

> "Run trusted hooks only
>
> Hooks execute local commands on your machine. Only install or create hooks from sources you
> trust, and review hook scripts before enabling them."

No mention was found of: a runtime approval/allowlist prompt before a hook first runs, a
signature/checksum mechanism, or config snapshotting hook definitions at session start (see
§10 for the related "does it need a restart" question, likewise undocumented). Contrast: the
same config.yaml does have an **unrelated** allowlist mechanism for MCP extensions
(`GOOSE_ALLOWLIST`, per §2's key inventory, and a dedicated
`block.github.io/goose/docs/guides/allowlist/`-style guide referenced in search results) — but
that is scoped to extensions, not hooks, and I did not verify it extends to plugin/hook
trust decisions.

## 10. Third-party installability

**Yes, realistically file-write-only.** The entire surface is:
1. Create a directory under the shared `.agents/plugins/<name>/` pool (project or user scope
   — the same pool grim already targets for skills, per the brief).
2. Write `plugin.json` (name/version/description) and `hooks/hooks.json` (the event map) —
   both plain JSON, matching grim's existing JSON-splicing capability with no new parser
   needed.
3. Optionally drop accompanying scripts under `scripts/` and make them executable.

No vendor CLI call, no UI step, and no cloud/account dependency were found anywhere in this
flow — the official example (`hello-hooks/README.md`) literally walks through `mkdir`/`cp`/
`chmod` as the install method.

**Restart/snapshot gotcha**: **NOT DOCUMENTED.** I found no statement that config or plugins
are snapshotted at startup, nor one guaranteeing hot-reload. Practically, goose CLI sessions
are typically short-lived process invocations (the example tells you to "run goose normally"
right after copying the plugin in), so the question mostly matters for the **desktop app**
(long-running process) — untested/unverified here.

**One caveat for "own and update idempotently"**: because identity lives at the
whole-plugin-directory level (§3), grim must fully own whichever directory name it chooses
(e.g. never write into a user's own hand-authored plugin folder) — but within a
grim-owned directory, install/update/remove is a plain directory write/rewrite/delete, no
merge logic required. The one place grim would need real JSON-splice discipline is
`~/.config/goose/settings.json`'s `disabledPlugins` array, if grim ever supports a
disable-without-uninstall verb.

## 11. Trampoline viability

**Favorable — no hard blockers found.** Concretely, for a hypothetical
`grim hook run --client goose --event <E>`:

- Handler is a **plain shell command string**, not a JS/TS module and not an in-process
  function — an external command works natively, no goose-specific runtime needed.
- Payload arrives as **JSON on stdin** — trivial to read generically.
- Response contract is **exit code + optional JSON on stdout**, exactly the shape a generic
  wrapper binary can produce (compute a decision, print `{"decision":"block","reason":"..."}`
  or exit 0/print nothing).
- `${PLUGIN_ROOT}` interpolation is resolved by goose before invocation, not something grim's
  trampoline needs to implement itself.

**Soft blockers / open questions, not hard stops**:
- The response contract is **binary** (allow/block) with no `allow`/`ask`/modify-input/
  add-context verbs — a portable grim hook schema modeled on a richer contract (e.g. Claude
  Code's) would have to gracefully degrade unsupported verbs to "allow" or "block" on this
  client, silently dropping the rest.
- No stable per-hook-entry id — grim's uninstall/update logic must operate at the
  plugin-directory granularity, not a finer-grained "just this one hook" granularity, if it
  wants provenance/idempotency guarantees.
- Working directory, `$PATH`, and stderr/stdout visibility to the end user are all
  **NOT DOCUMENTED**, so a trampoline binary can't yet rely on any particular cwd or assume
  its diagnostic output reaches a human.
- "Open Plugins" is Goose's own, not an externally-arbitrated, format (§1) — nothing stops a
  future Goose release from changing this shape unilaterally; there is no spec body to hold it
  stable the way there would be for, say, the Language Server Protocol.

---

## Sources

| URL | What it establishes | Fetched |
|---|---|---|
| https://block.github.io/goose/ | Old docs URL now shows a "goose has moved!" redirect banner to goose-docs.ai | 2026-08-14 |
| https://goose-docs.ai/blog/2026/04/07/goose-moves-to-aaif/ | Block donated goose to the Agentic AI Foundation (Linux Foundation), 2026-04-07; new org `aaif-goose`, new docs domain `goose-docs.ai` | 2026-08-14 |
| https://api.github.com/repos/block/goose | GitHub API ground truth: `full_name: aaif-goose/goose`, `homepage: https://goose-docs.ai/`, not archived, actively pushed as of fetch date | 2026-08-14 |
| Linux Foundation press release, "Linux Foundation Announces the Formation of the Agentic AI Foundation (AAIF)…" (linuxfoundation.org; mirrored prnewswire.com, aaif.io) | Independent corroboration of the AAIF/goose donation (announced Dec 2025); founding contributions MCP, goose, AGENTS.md | 2026-08-14 (via search) |
| https://goose-docs.ai/blog/2026/05/14/goose-hooks/ | Hooks feature announcement: concept, minimal `hooks.json` example, event list, `${PLUGIN_ROOT}`, fail-open behavior, `examples/plugins/hello-hooks` pointer | 2026-08-14 |
| https://goose-docs.ai/docs/guides/context-engineering/plugins/ | Plugin directory layout, `plugin.json` schema, discovery paths (`~/.agents/plugins/`, `<project>/.agents/plugins/`), alternate manifest locations, "Open Plugins" terminology | 2026-08-14 |
| https://goose-docs.ai/docs/guides/context-engineering/hooks | Full technical reference: event catalogue with firing conditions, input JSON schema + 3 verbatim examples, PreToolUse/Stop block contract, exit-code/JSON decision rules, fail-open wording, 30s timeout + override, `GOOSE_STOP_HOOK_BLOCK_CAP`, security warning quote, `disabledPlugins` pointer | 2026-08-14 |
| https://goose-docs.ai/docs/guides/config-files/ | config.yaml full key inventory (no hooks/plugins keys), default paths for macOS/Linux/Windows | 2026-08-14 |
| https://raw.githubusercontent.com/aaif-goose/goose/main/examples/plugins/hello-hooks/hooks/hooks.json | Verbatim real `hooks.json` (named-map shape, matcher regex, 4 events) | 2026-08-14 |
| https://raw.githubusercontent.com/aaif-goose/goose/main/examples/plugins/hello-hooks/plugin.json | Verbatim real `plugin.json` (name/version/description) | 2026-08-14 |
| https://raw.githubusercontent.com/aaif-goose/goose/main/examples/plugins/hello-hooks/README.md | Install-by-copy walkthrough; `disabledPlugins` lives in `~/.config/goose/settings.json` (JSON, separate from config.yaml) | 2026-08-14 |
| https://github.com/aaif-goose/goose/pull/9304 | Merged PR "feat(hooks): PreToolUse denial" — exit-code-2/`decision:block` deny protocol, sequential ordering + first-deny short-circuit, fail-open on any other failure, explicit statement of copying "Claude Code's hook conventions," `is_error:true` surfacing to avoid retry loops | 2026-08-14 |
| https://github.com/aaif-goose/goose/releases/tag/v1.34.0 | Hooks feature shipped in v1.34.0 (2026-05-13); "Install plugins to ~/.agents/plugins" (#9088) | 2026-08-14 |
| https://agent-plugins.org/ | The real, independently-launched "Agent Plugins" v1.0.0 cross-vendor spec explicitly excludes hooks from its portable core (client-specific reverse-domain namespaces only); TSC = Amazon/Cursor/Microsoft/OpenAI/Vercel — used here only to show it is **not** the same thing as Goose's "Open Plugins" term | 2026-08-14 |

### [unofficial] leads, not relied on as fact
- Search-engine synthesis (not independently re-fetched) placing "open-plugins generalization
  plus skills" work in releases v1.35.0 (2026-05-22) and v1.36.0 (2026-05-27) — plausible given
  v1.34.0 is the confirmed starting point, but not verified verbatim by me.
- A dev.to/mirror-site claim that `GOOSE_PATH_ROOT` overrides the XDG config path on Linux —
  plausible (matches the brief's own framing) but **not** found on any page I fetched directly
  from goose-docs.ai; flagged as unconfirmed-by-me rather than false.
- Numerous SEO/mirror domains (`block-goose.mintlify.app`, `instagit.com`, `contextqmd.com`,
  `awesome-goose.github.io`, `agent-safehouse.dev`, `deepwiki.com`) surfaced repeatedly in
  search results for this client; none were used as a factual basis for any claim above.
