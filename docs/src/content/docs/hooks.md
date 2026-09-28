---
title: Hooks reference
description: The gates a hook clears before it arms, workspace consent, the handler contract, and each client's limits. Experimental pre-1.0.
---
<!-- doc_type: reference -->

**Experimental pre-1.0**. See [Stability][stability-unstable].

A hook is a program a client runs before or after an agent's tool call. A
`hook.toml` published once is translated into each client's own hook format,
matcher syntax and JSON response. This page lists the gates a hook clears
before it arms, the contract its handler follows, and the limits of each
client.

[Control agent commands with hooks][guide] installs and writes one step by
step. [Artifact formats][artifacts-hooks] lists every manifest field. [When a
hook does not arm][troubleshooting] maps each warning to its fix.

## Arming gates {#why-gated}

A skill or a rule is text an agent reads. A hook executes, on every matching
tool call, on every client that armed it, with no confirmation step per call.
Grim's default for anything that executes is therefore to decline.

A declared hook arms only when all three gates pass:

| Gate | Passes when | `cause` when it holds the hook |
|---|---|---|
| Feature flag | `[options.experimental] hooks = true` in the scope's `grimoire.toml`. Off by default, with no environment variable to set it. | `feature-flag-off` |
| Workspace consent | This checkout has a consent record covering what it declares. See [Workspace consent][consent]. | `workspace-not-consented`, `consent-drifted` |
| Client capability | The client has a hook surface, accepts the hook's shape, and holds no file grim refuses to overwrite. | one per client, for example `surface-user-owned` |

A gate that holds a hook back declines to arm it at exit 0. Declaring a hook
the feature cannot run yet is never an error. `grim hook list` and
`grim status` report the gate as a `cause` token, listed in [Hook
arming][json-hook-arming].

## Workspace consent {#consent}

A repository's `grimoire.toml` is written by whoever committed it, so a
declaration alone never arms a hook. Consent is a separate, machine-local
record that a clone never carries.

Exactly three actions write a consent record:

- `grim hook allow`, run in the workspace.
- `grim add --kind hook`, because typing the reference is the declaration
  gesture.
- An accepted interactive prompt during `grim install`.

`grim install` and `grim update` never write one on their own. Without a
terminal to ask on, they arm nothing. `grim hook revoke` removes the record.

The record lives at `$GRIM_HOME/hooks/consent/<workspace-key>.json`, keyed by
the workspace path. Consent in one clone grants nothing to another clone, or to
the same clone on another machine.

The record names each hook binding and its source repository, without a tag or
digest. A version bump of a consented hook needs no new consent, because the
lock pin records which bytes arm. A new binding, or a hook from a new
repository, is drift, reported as `consent-drifted` until `grim hook allow`
covers it.

Global scope is always consented and writes no record. It has no shared
checkout a hostile declaration could arrive in.

### Per-run answers {#trust-flags}

`--trust-hooks` answers the consent question for one invocation of `add`,
`install` or `update`, and writes no record. `--no-trust-hooks` refuses for one
run. This is the form CI uses, since it has no terminal and keeps no record.

The pair outranks a stored record in both directions. `--no-trust-hooks`
refuses a consented workspace, and `--trust-hooks` arms an unconsented one.
Neither flag turns the feature flag on.

Neither answer can come from a file or an environment variable. A repository
routinely carries its own environment through `.envrc` or a devcontainer, and
an environment switch would let it consent on the cloner's behalf.

## Feature flag off {#flag-off-retention}

Setting `[options.experimental] hooks` back to `false`, then running
`grim install`, removes every client registration and dispatch row at that
scope. Each hook's state returns to `gated`, with cause `feature-flag-off`.

Three things stay, so that turning the flag back on costs nothing:

- the **payload**, the hook's unpacked files under `$GRIM_HOME`
- the **launcher** at `$GRIM_HOME/hooks/bin/grim-hook`, which other projects
  may still reference
- the workspace's **consent record**

With the flag back on, the next `grim install` arms the same hooks with no new
`grim hook allow`.

## Remove and uninstall {#remove-stays-armed}

| Command | Declaration and lock | Dispatch row and client registration | Payload |
|---|---|---|---|
| `grim remove hook <name>` | removed | kept until the next `grim install` or `grim uninstall` | kept |
| `grim uninstall hook <name>` | removed | removed | removed |

`grim remove` touches nothing on disk for any kind, and a hook is no
exception. It warns that the hook stays armed. The next `grim install`
converges the hooks to what is declared, and disarms it.

## Handler contract {#handler-contract}

A handler is the program a `[[hooks]]` entry names with `argv` or `command`. It
reads one JSON envelope on stdin and writes one JSON answer on stdout. The
contract below is the same on every client, because grim runs the handler and
translates its answer.

### Invocation {#handler-invocation}

The client runs grim's launcher, which runs `grim hook run`. Grim selects the
armed entries whose `event` matches and whose `matcher` admits the tool name.

- **Matcher.** An exact tool name, an `A|B` alternation, or a glob, never a
  regular expression. With no `matcher`, the entry runs for every tool call.
- **`argv`** runs in exec form, with no shell. Grim expands `${GRIM_HOOK_DIR}`
  and `$GRIM_HOOK_DIR` in each element itself.
- **`command`** runs through `/bin/sh -c`, or `cmd /C` on Windows.
- **Working directory** is the hook's own payload directory, so a relative
  path such as `approve.py` resolves to a file the hook ships.
- **stderr** is discarded.

### Stdin envelope {#handler-envelope}

One JSON object arrives on stdin, with these members in this order:

| Member | Value |
|---|---|
| `schema` | `1`, the same version as the manifest's `schema` |
| `event` | the canonical event: `PreToolUse`, `PostToolUse`, `SessionStart` or `Stop` |
| `native_event` | the client's own name for the same moment |
| `client` | grim's client name, for example `claude` |
| `scope` | `global` or `project` |
| `hook` | `<artifact>/<id>` |
| `tier` | `observer`, `gatekeeper` or `mutator` |
| `cwd` | the working directory the client reported |
| `session_id` | the client's session id, or `null` when it sends none |
| `correlation_id` | an id joining this invocation's audit records |
| `tool` | `{"name": …, "input": …}`, or `null` for an event without a tool |
| `raw` | the client's own payload, byte for byte |

`tool.input` is the client's tool input, unchanged, and `{}` when the client
sent none. A shell command is at `tool.input.command`. Read it by parsing the
envelope, never by searching the raw bytes for a substring.

With `payload = "file"` on the entry, grim also writes the envelope to a file
and names it in `GRIM_HOOK_PAYLOAD`. Stdin carries the envelope either way.

### Environment {#handler-environment}

Grim sets these variables, each a flat, non-secret value. Tool input never
travels in the environment.

| Variable | Value |
|---|---|
| `GRIM_HOOK_SCHEMA` | the envelope's `schema` |
| `GRIM_HOOK_EVENT` | the canonical event |
| `GRIM_HOOK_CLIENT` | the client name |
| `GRIM_HOOK_NAME` | `<artifact>/<id>` |
| `GRIM_HOOK_TIER` | the entry's tier |
| `GRIM_HOOK_TOOL` | the tool name only |
| `GRIM_HOOK_CWD` | the reported working directory |
| `GRIM_HOOK_DIR` | the hook's payload directory |
| `GRIM_HOOK_PAYLOAD` | the envelope file, set only with `payload = "file"` |

A value containing `{`, `}`, `[`, `]` or a control character is left unset,
with a warning.

### Response {#handler-response}

The handler prints one JSON object. Every member is optional, and unknown
members are ignored.

| Member | Type | Meaning |
|---|---|---|
| `decision` | `allow`, `deny`, `ask` or `none` | the verdict; `none` by default |
| `reason` | string | shown with the verdict |
| `context` | string | text added to the agent's context |
| `updated_input` | object | a replacement tool input, `mutator` at `PreToolUse` only |
| `user_message` | string | no client carries it, so grim drops it with a warning |
| `stop` | boolean | no client carries it, so grim drops it with a warning |

`{}` and `{"decision": "allow"}` are not the same answer. `{}` has no opinion,
and the client's own approval flow runs as usual. An explicit `allow` approves
the call, and on [Claude Code][claude] and [Copilot][copilot] it skips the
client's approval prompt.

Empty or whitespace-only output is no opinion. Output that does not parse as
JSON is no opinion plus a warning. Grim reads at most 64 KiB of stdout.

### Tiers {#handler-tiers}

| Tier | What counts from its answer |
|---|---|
| `mutator` | `updated_input`, at `PreToolUse` only. Mutators run first, one at a time in declaration order, each seeing the previous rewrite. |
| `gatekeeper` | `decision`, `reason` and `context`. Every gatekeeper judges the final, rewritten input. |
| `observer` | nothing. Observers run last, and their answers are discarded. |

A verdict from a non-gatekeeper, or a rewrite from a non-mutator, is ignored
with a warning. Across gatekeepers the strictest verdict wins, in the order
`none` < `allow` < `ask` < `deny`. The reason comes from the gatekeeper that
decided. A `deny` discards any rewrite.

### Exit code and timeout {#handler-exit}

Grim ignores the handler's exit code. A JSON answer on stdout counts whatever
the status, and a failed handler with no answer is no opinion. `grim hook run`
itself always exits `0`, so a client that fails closed on a non-zero hook exit
is never blocked by grim.

Each entry runs under its `timeout`, 30 seconds when unset. A handler still
running at its timeout is killed, and counts as no opinion. One that answered
and then kept running is killed, and its answer still counts.

All handlers for one tool call share one budget: the shortest `timeout` among
them, less half a second. A handler not started before the budget runs out is
skipped. Grim thereby answers before the client's own timeout.

### Client translation {#handler-translation}

Grim writes the combined answer in the invoking client's own response shape.
At `PreToolUse`:

| Client | Verdict field | `updated_input` |
|---|---|---|
| [Claude Code][claude] | `hookSpecificOutput.permissionDecision` | `hookSpecificOutput.updatedInput` |
| [Codex][codex] | `decision` (`approve` or `block`) and `hookSpecificOutput.permissionDecision` | `hookSpecificOutput.updatedInput` |
| [Copilot][copilot] | `hookSpecificOutput.permissionDecision` | `hookSpecificOutput.updatedInput` |
| [Qoder][qoder] | `hookSpecificOutput.permissionDecision` | not supported |

At `PostToolUse` and `Stop`, most clients accept only a blocking verdict. Where
a client cannot express a verdict at that event, grim adjusts it:

- `ask` becomes `deny`, with the reason attached.
- `allow` is dropped with a warning, and the client's own flow applies.
- A verdict or rewrite the client has no field for at all yields no answer,
  with a warning.

## Global-only clients {#global-only}

[Codex][codex], [Copilot][copilot] and [Qoder][qoder] arm hooks at global scope
only. Each one's project-scope hook file is tracked in the repository:

| Client | Project-scope hook file |
|---|---|
| [Codex][codex] | `.codex/hooks.json` |
| [Copilot][copilot] | `.github/hooks/*.json` |
| [Qoder][qoder] | `.qoder/settings.json` |

A registration written there travels with every clone, and grim never writes an
armable file into a repository. A project declaration therefore skips those
clients, with a warning pointing at `--global`.

[Claude Code][claude] arms at both scopes. Its project-scope surface,
`.claude/settings.local.json`, is a local override file that git ignores by
convention.

Each client's event set, and how far each was verified, is in the [client
matrix][clients-hooks].

### Copilot cloud agent {#copilot-cloud-agent}

[Copilot][copilot]'s CLI and IDE surfaces arm, at global scope. Its cloud agent
never does, and grim writes nothing under `.github/hooks/` for it.

The cloud agent reads hook config from `.github/hooks/*.json` on the pushed
branch it works on. A registration there runs unattended and server-side, on
whatever a contributor pushed.

## Plugin export {#export-declines}

[`grim export plugin`][cmd-export-plugin] renders locked artifacts into a
plugin a client installs without grim. A hook needs grim at run time, because
its launcher runs `grim hook run`.

Every hook member is therefore left out of an export. Each gets a warning, a
line in the plugin's README, and a row in the export report, at exit 0. An
export with nothing but hooks has nothing to render, and exits 65.

## See also

- [Control agent commands with hooks][guide] installs a published hook and
  writes one that approves safe commands.
- [When a hook does not arm][troubleshooting] maps each warning to its fix.
- [Artifact formats][artifacts-hooks] lists every `hook.toml` field.
- [Hook arming][json-hook-arming] lists every `cause` token.
- [Install order alongside a hook][mcp-install-order] covers a hook and an MCP
  entry sharing one client file.

<!-- internal -->
[stability-unstable]: ./stability.md#unstable
[guide]: ./guides/agent-hooks.md
[troubleshooting]: ./hooks-troubleshooting.md
[consent]: #consent
[artifacts-hooks]: ./artifacts.md#hooks
[json-hook-arming]: ./json-interface.md#hook-arming
[clients-hooks]: ./clients.md#gap-codex-hooks
[cmd-export-plugin]: ./commands.md#export-plugin
[mcp-install-order]: ./mcp-servers.md#install-order-hooks

<!-- external -->
[claude]: https://code.claude.com
[codex]: https://developers.openai.com/codex
[copilot]: https://github.com/features/copilot
[qoder]: https://qoder.com
