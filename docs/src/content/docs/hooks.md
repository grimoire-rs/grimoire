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

Hooks are workflow automation, not a policy control. A gatekeeper handler that
fails or times out is withheld, and the client's own policy then decides. A
removed or revoked hook is disarmed only at the next `grim install`. For
enforcement, use your client's managed settings.

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

Hooks arm for every hook-capable client on Linux and macOS. On Windows they
arm for Claude Code only. Windows also needs `GRIM_HOME` to be a local path
under `%USERPROFILE%` or `%LOCALAPPDATA%`. Any other home reports
`grim-home-shared`, and only on Windows.

Every other client, including Codex and Copilot, does not arm on Windows yet.
Grim arms a client there only after its own hook launch has been observed
working. Once the feature flag is on and the workspace has consented,
`grim hook list` reports the cause as `platform-unsupported`. Until then it
reports the gate that holds the hook back.

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
- the **launcher** at `$GRIM_HOME/hooks/grim-hook`, which other projects may
  still reference
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

The client runs grim's launcher, `grim-hook <client> <event> <root>`, a small
native program that grim writes to `$GRIM_HOME/hooks/grim-hook` on `grim
install`. It reads the dispatch table beside it and needs neither the `grim`
binary nor your config. The `<root>` operand is an opaque token that names the
workspace the registration came from. The launcher selects the armed entries
whose `event` matches and whose `matcher` admits the tool name.

- **Matcher.** An exact tool name, an `A|B` alternation, or a glob, never a
  regular expression. With no `matcher`, the entry runs for every tool call.
- **`argv`** runs in exec form, with no shell. Grim expands `${GRIM_HOOK_DIR}`
  and `$GRIM_HOOK_DIR` in each element itself.
- **`command`** runs through `/bin/sh -c` on Linux and macOS. On Windows it
  runs through `cmd /C`.
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

Empty or whitespace-only output from a handler that exited 0 is no opinion.
Output that does not parse as JSON is no opinion plus a warning, and it
withholds a gatekeeper. Grim reads at most 64 KiB of stdout.

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

Grim does not read the handler's exit code as a verdict. A JSON answer on
stdout counts whatever the status. A failed handler with no answer is no
opinion for an observer or a mutator.

A gatekeeper that fails without an answer is withheld instead. That covers a
crash with empty stdout, a timeout, unparsable output and a handler that cannot
be spawned. The launcher then exits `1`, and the client's own policy decides.

A composed `deny` or `ask` is never replaced by that exit. Claude Code treats
an error exit as non-blocking, so the launcher keeps the verdict it composed.

Each entry runs under its `timeout`, 30 seconds when unset. A handler still
running at its timeout is killed, and counts as a failure with no answer. One
that answered and then kept running is killed, and its answer still counts.

Every hook matched for one tool call shares one budget, so grim answers before
the client's own timeout. Grim registers hooks with the same matcher as one
client entry, whose timeout is the longest `timeout` among them. The budget is
the shortest of those entries' timeouts, less half a second. An observer that
shares a gatekeeper's matcher therefore never cuts it short. A handler still
running when the budget runs out is cut off, and one not yet started is
skipped.

`timeout` lies between 1 and 3600 seconds. `grim build` refuses a value
outside that range with exit code 65. Below 1 second the hook has no time to
start, and no client waits an hour for a hook.

### Launcher exit codes {#launcher-exit}

The launcher never forges a verdict. Every state that is not a failure exits
`0`, with an answer on stdout when a hook gave one. These states are the
feature flag off, a revoked workspace, an unknown root, and an event no entry
matches.

A failure in grim itself exits non-zero instead. The client's own policy for a
failing hook then decides what happens to the tool call. The launcher never
exits `2`, because Claude Code reads that status as a block.

| Exit | Meaning |
|---|---|
| `0` | Success, or nothing to do. |
| `1` | Any other internal failure, such as a gatekeeper's envelope that could not be built. Also a gatekeeper whose handler gave no usable verdict. |
| `64` | The three operands are missing or malformed. |
| `65` | The dispatch table is unparsable, has a schema this launcher does not know, or a selected entry is unreadable. Also a client payload over the size cap, or one that is not a JSON object while a gatekeeper is selected. |
| `74` | An I/O failure: the launcher cannot find its own path, or the table or stdin cannot be read. Also when a gatekeeper cannot run because grim could not write its audit record, or could not create or write its `payload = "file"` envelope. |
| `79` | `dispatch.json` is absent while a registration still fires. |

What a non-zero exit does depends on the client, and grim does not change it.
[Codex][codex] marks the hook as failed and proceeds. [Copilot][copilot] denies
a `preToolUse` call and lets other events through.

[Claude Code][claude] shows a non-blocking error and the action proceeds. The
exception is a hook that runs through PowerShell on Windows, which blocks. See
[When a hook does not arm][troubleshooting]. [Qoder][qoder] is unverified.

Deleting the launcher or the dispatch table while a hook is still registered
makes every call fail by the client's policy. That lasts until the next
`grim install` writes them again. An observer's or a mutator's own failure is
never in this table. It gives no opinion.

The launcher reads one environment variable, `GRIM_LOG`, for verbosity only.
Warnings reach stderr at any setting. A value containing `debug` or `trace`
adds the debug lines. It is not a tracing filter: a target filter such as
`grim=debug` counts only because it contains `debug`.

### Client translation {#handler-translation}

Grim writes the combined answer in the invoking client's own response shape.
At `PreToolUse`:

| Client | Verdict field | `updated_input` |
|---|---|---|
| [Claude Code][claude] | `hookSpecificOutput.permissionDecision` | `hookSpecificOutput.updatedInput` |
| [Codex][codex] | `decision` (`approve` or `block`) and `hookSpecificOutput.permissionDecision` | `hookSpecificOutput.updatedInput` |
| [Copilot][copilot] | `hookSpecificOutput.permissionDecision` | `hookSpecificOutput.updatedInput` |
| [Qoder][qoder] (unverified, no live firing test yet) | `hookSpecificOutput.permissionDecision` | not supported |

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

Qoder is unverified: no Qoder hook has been seen firing from a grim
registration yet. See [Qoder: hooks unverified by a live run][clients-qoder].

[Claude Code][claude] arms at both scopes. Its project-scope surface,
`.claude/settings.local.json`, is a local override file that git ignores by
convention.

Grim writes a settings file that is a symlink, such as a dotfiles checkout,
through the link, and the link stays. At project scope it refuses a settings
file that resolves outside the workspace, whether the file or its `.claude`
directory is the link. A cloned repository could otherwise aim the write at
another project's settings. That hook reports `not-armed` with cause
`not-registered`.

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
plugin a client installs without grim, and `grim export marketplace` renders
the same trees for a marketplace repository. A hook needs grim at run time,
because its launcher and the dispatch table it reads live under `$GRIM_HOME`
on the machine where grim installed them.

Every hook member is therefore left out of an export. Each gets a warning, a
line in the plugin's README, and a row in the export report, at exit 0. An
export with nothing but hooks has nothing to render, and exits 65. In a
marketplace, that applies to each plugin on its own.

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
[clients-qoder]: ./clients.md#gap-qoder-hooks
[cmd-export-plugin]: ./commands.md#export-plugin
[mcp-install-order]: ./mcp-servers.md#install-order-hooks

<!-- external -->
[claude]: https://code.claude.com
[codex]: https://developers.openai.com/codex
[copilot]: https://github.com/features/copilot
[qoder]: https://qoder.com
