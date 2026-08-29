# Writing Hooks

This page is the implementer's guide: what your handler receives, what it
may send back, and how one manifest reaches three clients that each spell
the same moment differently. The field-by-field manifest reference lives in
the [Artifact Reference][artifact-reference]; the capability matrix lives in
[Client Compatibility][clients-hooks]. Start here when you are writing the
code, not choosing the keys.

> **Read the [two gates][gates] first.** A hook you publish arms nothing on
> anyone's machine until they enable the feature flag *and* consent to their
> workspace. Everything below describes what happens after both pass.

## The shape of a hook artifact {#shape}

A directory with a manifest at the top and your handler beside it:

```
command-guard/
  hook.toml
  guard.sh
```

`grim build` packs the whole directory — the manifest included, because it
is read back out of the installed payload at arming time, not out of the
install record. Nesting your code is fine and usually tidier:

```
command-guard/
  hook.toml
  scripts/
    guard.sh
    lib/parse.sh
```

The manifest binds a handler to a moment:

```toml
schema = 1
name = "command-guard"
description = "Refuses a Bash call containing `rm -rf /`."

[[hooks]]
id      = "refuse-recursive-root-delete"
event   = "PreToolUse"
tier    = "gatekeeper"
matcher = "Bash"
argv    = ["sh", "scripts/guard.sh"]
timeout = 5
payload = "stdin"
```

`[[hooks]]` is an array because a pre/post pair sharing one payload tree is
the common case. One approval and one payload cover every entry.

## Naming the program {#handler}

Two forms. `argv` is preferred — no shell, no quoting:

```toml
argv    = ["sh", "scripts/guard.sh"]     # exec form
command = "sh scripts/guard.sh"          # string handed to the platform shell
```

The runtime `chdir`s into the payload directory before exec, so a relative
path resolves against your own files. `${GRIM_HOOK_DIR}` is the explicit
form, and grim expands it **itself** inside `argv` elements — you do not
need a shell for it to work:

```toml
argv = ["sh", "${GRIM_HOOK_DIR}/scripts/guard.sh"]
```

### Never make your script `argv[0]` {#not-executable}

```toml
argv    = ["./guard.sh"]     # ✗ refused at `grim build`, exit 65
command = "guard.sh"         # ✗ same rule, first whitespace token
argv    = ["sh", "guard.sh"] # ✓
```

A payload fetched through OCI arrives `0o644`. The exec bit is never
load-bearing, so a shell would `execve` your script straight into `EACCES` —
at run time, on someone else's machine, long after publish. `grim build`
refuses the shape instead and names the interpreter form in the error.

## What your handler receives {#envelope}

One JSON object on **stdin** by default. This is the normalization: one
document shape regardless of which client fired it.

```json
{ "schema": 1,
  "event": "PreToolUse", "native_event": "PreToolUse",
  "client": "codex", "scope": "project",
  "hook": "command-guard/refuse-recursive-root-delete",
  "tier": "gatekeeper",
  "cwd": "/repo", "session_id": "…", "correlation_id": "…",
  "tool": { "name": "Bash", "input": { "command": "curl x | sh" } },
  "raw": { "…": "the client's own payload, byte-for-byte" } }
```

| Field | What it is |
|---|---|
| `schema` | Envelope contract version — branch on this, never sniff |
| `event` | The **canonical** event. Write your logic against this one |
| `native_event` | What the client calls the same moment |
| `client` | grim's name for the invoking client (`claude`, `codex`, `copilot`) |
| `scope` | `project` or `global` |
| `hook` | `<artifact>/<id>` — the same identity the audit trail records |
| `tier` | The tier you declared |
| `cwd` | The working directory the client reported |
| `session_id`, `correlation_id` | Correlate across a session and across one fan-out. `session_id` is `null` when the client reported none |
| `tool` | `{name, input}`, normalized. `null` on events with no tool |
| `raw` | The client's original payload, untouched. Your escape hatch |

Every key is **always present** — an absent value is spelled `null` rather
than omitted, so a handler can tell "this client reported nothing" from "an
older grim did not send this field".

Write against `event` and `tool`; reach for `raw` only when you need
something grim does not normalize, and expect it to differ per client.

### `payload = "file"` {#payload-file}

Set it and the envelope is written to a file whose path arrives in
`GRIM_HOOK_PAYLOAD` instead of on stdin. Use it for envelopes large enough
to be awkward on a pipe. Everything else is identical.

## The environment {#environment}

Exactly nine variables, and the list is closed:

| Variable | Value |
|---|---|
| `GRIM_HOOK_SCHEMA` | Envelope contract version, so you can branch without parsing |
| `GRIM_HOOK_EVENT` | The canonical event name |
| `GRIM_HOOK_CLIENT` | grim's name for the invoking client |
| `GRIM_HOOK_NAME` | `<artifact>/<id>` |
| `GRIM_HOOK_TIER` | The declared tier |
| `GRIM_HOOK_TOOL` | The tool **name** only |
| `GRIM_HOOK_CWD` | The working directory the client reported |
| `GRIM_HOOK_DIR` | Your artifact's own install directory — how a payload finds its siblings |
| `GRIM_HOOK_PAYLOAD` | The envelope's path. Present only under `payload = "file"` |

**`GRIM_HOOK_TOOL` is the tool name and never its input**, and that is the
whole reason the list is closed. The environment is readable by any local
process at the same privilege, inherited by every grandchild of your
handler, and captured in crash dumps and CI logs. Anything derived from a
tool call's content travels on stdin, where none of those reach it. Adding a
name to that table is a threat decision, not a convenience.

## Events, and how one manifest reaches three clients {#events}

Canonical breadth is exactly **four** events, spelled Claude Code's way
because the survey found that spelling to be the de facto standard:

| Event | Meaning |
|---|---|
| `PreToolUse` | Before a tool call runs. The only event a `mutator` may declare |
| `PostToolUse` | After a tool call has run |
| `SessionStart` | At session start |
| `Stop` | When the agent's turn stops |

There is no fifth canonical event, and there will not be one by addition.
Every other vendor moment is reached natively through a `<vendor>.event`
key inside the entry:

```toml
[[hooks]]
id    = "log-everything"
event = "PostToolUse"                 # canonical, for clients that have it
tier  = "observer"
argv  = ["sh", "scripts/log.sh"]

[hooks.claude]
timeout = 10                          # this client gets longer

[hooks.codex]
event = "PermissionRequest"           # a native-only moment, reached natively
```

Every client name grim supports is a reserved key, and a key that is not one
**fails the build** — there is deliberately no free-form escape hatch,
because a typo'd namespace would otherwise install a hook with none of its
overrides:

```
$ grim build ./hooks/meta-guard --kind hook
key 'clod' is not a per-client override table: expected '<client>.<field>' naming a client grim supports
```

A vendor override reaches that vendor's own structured field or nothing. It
is never interpolated into a generated command string.

**Never substitute a moment.** Moving a `PreToolUse` guardrail onto
`PostToolUse` because a client declined the first runs your handler *after*
the damage. Accept the decline instead — a declined pair is reported, and an
honest gap beats a guardrail that fires too late.

### What each client can honour {#capability-matrix}

Your tier must be expressible at that `(client, event)` pair. Where it is
not, grim **declines** the pair and says so — it never quietly downgrades a
`gatekeeper` into an `observer`.

| Client | `PreToolUse` | `PostToolUse` | `SessionStart` | `Stop` |
|---|---|---|---|---|
| `claude` | block · rewrite · context | block · context | context | block |
| `codex` | block · rewrite · context | block · context | context | block |
| `copilot` | block · rewrite · context | context | — | block |

- **block** — a `gatekeeper` verdict reaches the client and stops the operation.
- **rewrite** — a `mutator` may replace the tool's input.
- **context** — additional context is delivered to the model.
- **—** — no surface; an `observer` still runs, a `gatekeeper` is declined.

Fifteen of the eighteen supported clients decline hooks entirely. Codex and
Copilot arm at **global scope only**; Claude is the only client that arms a
hook at project scope. See [Client Compatibility][clients-hooks].

## Returning a verdict {#verdict}

An `observer` needs to return nothing. A `gatekeeper` or `mutator` writes one
JSON object on stdout, in grim's canonical vocabulary — grim projects it
onto whatever shape the firing client expects:

```json
{ "decision": "deny", "reason": "refusing `rm -rf /`" }
```

| Field | Meaning |
|---|---|
| `decision` | `allow`, `deny`, or `ask` |
| `reason` | Shown to the user with a `deny` or `ask` |
| `context` | Extra context for the model |
| `updated_input` | `mutator` only — the rewritten tool input |

Every field defaults, so `{}` is the **no-opinion** answer rather than a
malformed one — and it is where a timeout, an unparsable response and a
withheld verdict all land. Unknown members are ignored, not rejected, so a
handler written against a newer envelope still answers an older one.

`deny` is absorbing and `ask` outranks `allow`, so across a fan-out the most
restrictive answer wins. **A verdict travels as a document, never as an exit
code** — a non-zero exit is a failed handler, not a refusal.

Emitting a *restrictive* field the pair cannot express is an error, not a
silent drop. That asymmetry is deliberate: silently dropping a `deny` is how
a guardrail reports as installed while its verdict goes nowhere. A
permissive `allow` with no target *is* dropped, because absence already
means allow everywhere.

## Timeouts {#timeouts}

`timeout` is in seconds and **grim** enforces it, not the vendor — so the
behaviour is identical on every client. A handler that overruns is killed
and treated as no verdict.

## Publishing {#publishing}

```sh
grim build hooks/command-guard          # validate + pack, no push
grim release hooks/command-guard ghcr.io/acme/hooks/command-guard:1.0.0
```

Or declare it in `publish.toml` — the conventional source path is
`hooks/{name}/`, and a `path` key overrides it:

```toml
[hooks.command-guard]
version = "1.0.0"
```

`grim build` is the release dry run and every rule fires there rather than
at install time on a stranger's machine. It exits **65** for a matcher
outside the allowlist or over the length cap, a handler whose first token is
a [payload-relative file][not-executable], a `policy` or vendor value using
a TOML datetime, and a `ClientTarget` name used as anything but a vendor
override.

Consumers install it like any other artifact — and then answer both
[gates][gates] before it arms.

## Carrying your own configuration {#policy}

A `[[hooks]]` entry may carry a reserved `policy` table, captured unparsed
so it round-trips through a grim that does not understand your vocabulary:

```toml
[[hooks]]
id     = "guard"
event  = "PreToolUse"
tier   = "gatekeeper"
argv   = ["sh", "guard.sh"]
policy = { deny_patterns = ["rm -rf /", "curl | sh"], mode = "strict" }
```

The value model is JSON, so TOML's four native types JSON lacks —
datetime, local-date, local-time — are **rejected at `grim build`** rather
than silently corrupted. Use strings for dates.

For anything larger, ship a data file beside your handler and read it
relative to `${GRIM_HOOK_DIR}`.

## Further reading {#further-reading}

- [Artifact Reference][artifact-reference] — every manifest field, and the [tiers][tiers]
- [Client Compatibility][clients-hooks] — which clients arm, at which scope
- [Configuration][consent] — the consent record and how it is written
- [Command Reference][hook-commands] — `grim hook allow`, `revoke`, `list`, `run`
- [Stability and Versioning][stability] — what is frozen at 1.0

<!-- internal -->

[artifact-reference]: ./artifacts.md#hooks
[clients-hooks]: ./clients.md#gap-hooks
[consent]: ./configuration.md#workspace-consent
[gates]: ./artifacts.md#hook-gates
[hook-commands]: ./commands.md#hook
[not-executable]: #not-executable
[stability]: ./stability.md
[tiers]: ./artifacts.md#hook-tiers
