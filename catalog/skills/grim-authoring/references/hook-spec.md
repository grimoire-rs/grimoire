# Hook Spec

You loaded this file because you are authoring or fixing a grim **hook** —
a directory holding a `hook.toml` manifest plus the payload files its
handlers run — for `grim build` or `grim release`.

Contents: [Ships Disarmed](#ships-disarmed) · [File Shape](#file-shape) ·
[Top-Level Keys](#top-level-keys) · [The `[[hooks]]` Entry](#hooks-entry) ·
[Events](#events) · [Tiers](#tiers) · [Matchers](#matchers) ·
[The Handler](#the-handler) ·
[The Payload-Not-Executable Trap](#payload-not-executable) ·
[Vendor Overrides](#vendor-overrides) · [Reserved Names](#reserved-names) ·
[Which Clients](#which-clients) · [Publishing](#publishing) ·
[Validation Pitfalls](#validation-pitfalls)

## Ships Disarmed

Read this before you author anything. A published hook is not a published
*running* thing:

- Hooks are behind an experimental flag, **off by default**:
  `options.experimental.hooks` (config-only, no environment override).
- Enabling the flag is not consent to run your hook: whether the consumer's
  *workspace* has consented is a second, independent question, answered by
  `grim hook allow`, by `grim add`, or by an accepted prompt — and never by
  anything that travels in the repository.
- A consumer who adds your hook today gets a declaration, a lock pin, and a
  materialized payload tree. Consumers see what they have with
  `grim hook list`.

So write the `description` for someone reading a catalog listing, not for
someone who just had their tool call blocked — and never tell consumers a
hook fires on install.

Everything below is the **manifest contract**: what `grim build` accepts and
refuses, all of it verifiable on your own machine with `grim build`. Where a
line describes what a *client* does with an entry (a tier declined, a matcher
translated, a timeout enforced), read it as the contract the format
specifies, and confirm the live behaviour against the binary before relying
on it — do not restate any of it to consumers as observed behaviour.

## File Shape

A hook is a **directory** artifact, like a skill:

```
hooks/shell-guard/
├── hook.toml       # the manifest — this file is what makes it a hook
└── guard.sh        # payload: whatever the handlers run
```

Kind inference reads the directory for `hook.toml` **before** it looks for
`SKILL.md`, so a directory carrying both is a hook. `--kind hook` is
accepted on `build` and `release` but never required:

```
$ grim build ./hooks/shell-guard
Kind  Name         Path                    Layer Digest         Status
hook  shell-guard  ./hooks/shell-guard     sha256:bdbbc6cc…     built
```

The directory name is the artifact name, and `hook.toml`'s `name` must
equal it — same rule as a skill's `SKILL.md`.

## Top-Level Keys

`hook.toml` is a **strict** document (`deny_unknown_fields`) with exactly
four top-level keys:

| Key | Required | Notes |
|---|---|---|
| `schema` | yes | The manifest/envelope contract version. `1` today. A different integer fails with an explanatory error, never a bare parse failure |
| `name` | yes | Must equal the directory stem |
| `description` | yes | Becomes `org.opencontainers.image.description` |
| `[[hooks]]` | array | The declared handlers. Each is one entry; see below |

**A hook has no catalog-metadata surface.** `summary`, `keywords`,
`repository`, `deprecated`, and `replaced-by` are each a hard parse error
here (exit 65) — the strict schema means they fail loudly rather than
being silently dropped, which is the opposite of the skill/rule/agent
asymmetry. `description` is the only catalog-facing field a hook has, so
write it to carry the whole blurb.

Author in the **TOML 1.0-compatible subset** — unquoted dotted keys and
single-line inline tables only. Grim's own parser accepts more, but
`hook.toml` is a published format that third-party TOML 1.0 parsers read.

## The `[[hooks]]` Entry {#hooks-entry}

`[[hooks]]` is an array of tables because a pre/post pair sharing one
payload tree is the common case. One approval and one payload tree cover
every entry in the artifact.

| Field | Required | Notes |
|---|---|---|
| `id` | yes | Stable, unique within the artifact. **ASCII letters, digits, `_`, `-` and `.` only, max 128 bytes** — it reaches the audit trail as `<artifact>/<id>`, so it has to stay a single readable token |
| `event` | see [Events](#events) | A canonical event name. Omit it only when a `<vendor>.event` override stands alone |
| `tier` | yes | `observer`, `gatekeeper`, or `mutator` |
| `matcher` | no | Grim's own glob dialect — never a regex. Absent means every tool |
| `argv` **or** `command` | yes, exactly one | The program to run. See [The Handler](#the-handler) |
| `timeout` | no | Seconds; **default 30**. The format names *grim* as the enforcer rather than the vendor, so the value means the same thing on every client and a vendor's own timeout is only a backstop |
| `payload` | no | `stdin` (default) or `file` |
| `policy` | no | Reserved. Stored unparsed and round-tripped, so a future policy vocabulary lands without invalidating your artifact |
| `<client>.<field>` | no | Per-vendor override tables — see [Vendor Overrides](#vendor-overrides) |

`payload = "stdin"` puts one JSON envelope object on the handler's stdin
and is the default for a reason: it is the only transport that avoids the
always-on metadata leaks (`/proc/<pid>/cmdline`, `/proc/<pid>/environ`,
crash dumps, CI logs). `payload = "file"` writes the envelope to a file
whose path is exported as `GRIM_HOOK_PAYLOAD` — an explicit opt-in for
envelopes that would overflow `ARG_MAX`, never a default. Never put a
secret in `argv` or in an environment variable your handler reads.

## Events

Canonical breadth is exactly **four** events, spelled Claude Code's way
because the survey found that spelling to be the de facto standard:

| Event | Meaning |
|---|---|
| `PreToolUse` | Before a tool call runs. The only event a `mutator` may declare |
| `PostToolUse` | After a tool call has run |
| `SessionStart` | At session start |
| `Stop` | When the agent's turn stops |

There is no fifth canonical event, and there never will be one by
addition: every other vendor moment is reached natively through a
`<vendor>.event` key inside the entry.

**Never substitute a moment.** Relocating a `PreToolUse` guardrail onto
`PostToolUse` because the target client's `PreToolUse` declines it runs the
handler *after* the damage. Accept the decline instead.

## Tiers

> **A grim hook is defence in depth, never a security boundary.** A
> `gatekeeper` that does not fire — because grim is not installed, the
> launcher is missing, the client never registered it, or the user's shell
> cannot run the registration — is *by design*: every layer fails open, so a
> broken guardrail can never deny someone a tool call. Write a `gatekeeper`
> to catch mistakes, and never as the control that has to hold.
>
> This is not a caveat on an otherwise-strong mechanism; it is the mechanism.
> The registration itself begins `[ -f "$L" ] && [ -x "$L" ] || exit 0`, and a
> dispatch table grim cannot parse disarms every hook rather than blocking
> anything.

The tier is a **capability declaration**, and it is resolved per client. A
client that cannot honour a tier declines the `(hook, client)` pair — it is
never silently degraded into a weaker tier, because degrading a guardrail
into a logger would report it as installed while it can only watch.

| Tier | May do | Valid at |
|---|---|---|
| `observer` | Read the event; its response changes nothing | every event |
| `gatekeeper` | Return a verdict that blocks the operation | only events where some client can express a verdict |
| `mutator` | Rewrite the tool input | only `PreToolUse` — nothing later has an input left to rewrite |

Both restrictions are enforced at `grim build`, and the messages name the
pair:

```
$ grim build ./hooks/meta-guard --kind hook
tier 'mutator' is not valid at event 'PostToolUse'

$ grim build ./hooks/meta-guard --kind hook
tier 'gatekeeper' is not valid at event 'SessionStart'
```

An entry whose *only* moment is a `<vendor>.event` override has no
canonical event, so the table above cannot judge it. Such an entry may
declare `observer` or `gatekeeper` — `mutator` is refused outright,
because the tier is defined by `PreToolUse`:

```
hook 'native-only' declares tier 'mutator' on a native-only event: 'mutator' requires event = "PreToolUse"
```

## Matchers

A `matcher` is an exact tool name or a glob in grim's own dialect,
translated into each target client's dialect at registration time. It is
**not a regex**, and the charset is an allowlist — `[A-Za-z0-9_*?./-|]`,
at most 256 bytes:

```
$ grim build ./hooks/meta-guard --kind hook
invalid matcher '^Bash$': expected only [A-Za-z0-9_*?./-|]

$ grim build ./hooks/meta-guard --kind hook
matcher of 257 bytes exceeds the 256-byte limit
```

An allowlist rather than a denylist of quotes and control characters is
deliberate: a denylist still admits bidi and homoglyph characters that let
a matcher spoof what an approval prompt displays.

The format documents three forms as translating losslessly to every client
that hosts hooks — an exact name (`Bash`), a `*` glob, and `A|B`
alternation — so those are the safe choices; anything else risks a
per-client decline. An **empty** matcher is refused at build, because Claude
reads it as match-all while Copilot rejects it outright, so no translation is
both faithful and non-skipped. Omit the key instead of writing
`matcher = ""`.

## The Handler

Exactly one of `argv` or `command` — and prefer `argv`:

```toml
argv    = ["sh", "${GRIM_HOOK_DIR}/guard.sh"]   # preferred: no shell, no quoting
command = "sh guard.sh"                          # single string handed to the platform shell
```

`${GRIM_HOOK_DIR}` is exported as the artifact's own installed payload
directory, so it is how a handler names a file it shipped with.

Supplying **both** keys is refused, and this refusal earns its place:
without it the entry would parse cleanly with `argv` winning silently,
while a human reading the file bottom-up would believe `command` runs — a
shadow handler.

```
$ grim build ./hooks/meta-guard --kind hook
hook 'obs' declares both 'argv' and 'command'; exactly one of them is required

$ grim build ./hooks/meta-guard --kind hook
hook 'obs' declares no handler: set exactly one of 'argv' or 'command'
```

## The Payload-Not-Executable Trap {#payload-not-executable}

**This is the mistake to internalize.** A handler whose first token names a
file inside your payload tree is rejected at `grim build`:

```
$ grim build ./hooks/bad-guard --kind hook
hook 'direct-exec' runs the payload file './guard.sh' directly; a payload delivered
through a registry is not executable — name an interpreter instead, e.g.
argv = ["sh", "${GRIM_HOOK_DIR}/./guard.sh"]
$ echo $?
65
```

The reason is structural, not a policy choice: a payload fetched through an
OCI registry arrives mode `0o644`. The exec bit is never load-bearing for a
grim-delivered file, so a shell asked to `execve` your script would get
`EACCES` — at hook-fire time, on a consumer's machine, which is the worst
possible place to discover it. Failing at build on the publisher's machine
is the whole point.

So: **name the interpreter.**

```toml
argv = ["sh", "${GRIM_HOOK_DIR}/guard.sh"]        # correct
argv = ["python3", "${GRIM_HOOK_DIR}/guard.py"]   # correct
command = "./guard.sh"                             # refused, exit 65
argv = ["${GRIM_HOOK_DIR}/guard.sh"]               # refused, exit 65
```

Do not try to defeat the check by shipping the file with an exec bit set,
and do not `chmod +x` from a `SessionStart` hook. The check fires on the
first token whether it is written bare, `./`-prefixed, or
`${GRIM_HOOK_DIR}`-prefixed. A token that is absolute, or carries a `..`
component, is not payload-relative by definition and passes — that is an
interpreter path as far as this rule is concerned.

## Vendor Overrides

Per-client override tables are named for the client and captured verbatim:

```toml
[[hooks]]
id      = "obs"
event   = "Stop"
tier    = "observer"
argv    = ["sh", "${GRIM_HOOK_DIR}/log.sh"]

[hooks.claude]
timeout = 10

[hooks.codex]
event = "PermissionRequest"   # a native-only moment, reached natively
```

Unknown top-level entry keys are *not* denied — the format has to
round-trip a `<vendor>.<field>` table and the reserved `policy` key through
a grim that does not understand them. What closes the hole instead is
validation: every client name grim supports is a reserved key, and a key
that is not one fails the build. There is deliberately no free-form escape
hatch, because a typo'd namespace would otherwise install a hook with none
of its overrides:

```
$ grim build ./hooks/meta-guard --kind hook
key 'clod' is not a per-client override table: expected '<client>.<field>' naming a client grim supports
```

A vendor override reaches that vendor's own structured field or nothing —
it is never interpolated into a generated command string.

## Reserved Names

A hook's binding name becomes a directory under `$GRIM_HOME`, so it must be
a **plain artifact name** — lowercase alphanumerics with `.` or `-`
separators, and nothing else. A name carrying a path separator, a `..`
segment, or a drive letter is refused before anything is written.

On top of that, **five** names are refused outright: `bin`, `dispatch.json`,
`dispatch.json.lock`, `payload`, and `root-key`. Each names part of grim's own
launcher namespace
(`$GRIM_HOME/hooks/{bin,dispatch.json,dispatch.json.lock,payload,root-key}`), and
a payload materialized over one would arm or disarm the dispatcher itself.
`root-key` is the sharpest: it holds the machine's HMAC key, and a directory in
its place means no dispatch table can be written at all, so every hook on the
machine reports installed while nothing fires. `dispatch.json.lock` — the table's
advisory-lock sidecar — is the same failure with a warning attached, and it is
recoverable by uninstalling the offending artifact.

**The name rules are re-checked at install, not only at build.** `grim build`
refuses an unusable manifest `name`, but a hook published *before* that gate
existed carries one anyway, so the installer re-validates the installed
`hook.toml` and declines to arm rather than trusting the publisher. The warning
names the artifact and the reason:

```
WARN hook 'shell-guard' is not armed: its installed hook.toml does not satisfy the
     rules `grim build` enforces, so it was published without them: hook artifact
     name 'my_hook' is not usable: …
```

If you built a hook against an early build of this feature and it stops arming
with no config change on your side, this is why — rename the manifest `name` to a
plain name and republish. Nothing about the binding name in your `grimoire.toml`
needs to change.

If you are adding a file under `$GRIM_HOME/hooks/` in grim itself, add its name
to `RESERVED_ARTIFACT_NAMES` in the same change — unless the name is
unrepresentable as a binding name, which an underscore achieves (`hook_audit.jsonl`
and the transient `payload_<pid>_<slot>.json` envelopes are safe that way, not by
being reserved). This list has silently fallen behind the layout twice.

`hook_dispatch`'s `every_grim_owned_name_under_hooks_is_a_reserved_binding_name`
enforces it from the other side by listing the real directory after provoking
install's writes, so it catches a new file **that install writes** whether or not
anyone tells it about one. Its reach stops there: a file written by the runtime
path (the audit trail, a payload envelope) is invisible to it. Those names are
covered by a companion test,
`every_runtime_written_name_at_the_hooks_root_is_unusable_as_a_binding`, which
lists them one by one — so if you add a **new** runtime-side writer, add a row to
it, because neither test will catch you.

```
$ grim build ./hooks/bin --kind hook
hook artifact name 'bin' is reserved: 'bin' names part of grim's own hook launcher under $GRIM_HOME/hooks/ — rename the artifact
$ echo $?
65
```

The `payload` entry joined the set when the payload tree moved to
`$GRIM_HOME/hooks/payload/<workspace-key>/<name>/`; `root-key` joined it in
round 2 of review, having been missed when that file was added.

The reserved check is exact string equality against that list. `Bin` is
refused as well, but by the plain-name rule above — a plain name is
lowercase — so the two rules together leave no spelling that reaches the
launcher namespace. (No count is restated here on purpose: a second, stale
count 46 lines below the first is how this section last went wrong.)

Both rules are enforced at `grim build`, not only at install: a name every
consumer would refuse should not be publishable in the first place.

## Which Clients

Hooks are the narrowest kind grim publishes. Only **Claude**, **Codex**,
and **Copilot** name a hook registration surface at all, and only Claude
hosts one at *project* scope — Codex's and Copilot's registration files are
tracked repository files, so grim writes them at global scope only. Every
other client declines hooks.

Treat the [enforced matrix][clients] as authoritative. Design for the
decline: a hook whose value depends on firing everywhere is the wrong
artifact, and a skill reaches clients a hook never will.

## Publishing

- **A hook has no local-path source.** The config parser rejects a path
  value under `[hooks]`, and there is no dev-install for the kind — a
  dev-install's source is a working-tree path, and the natural "edit in
  the repo, re-install" loop would put something armable inside a
  repository. The loop is `grim build` → `grim release` (a local registry
  is fine for iterating) → `grim add` from that reference.
- **A hook can be a bundle member.** Add a `[hooks]` table to the bundle
  `.toml` alongside `[skills]`, `[rules]`, and `[agents]`:

  ```toml
  [hooks]
  shell-guard = "ghcr.io/acme/hooks/shell-guard:1"
  ```

  Members sort by kind on the wire and a hook sorts last, so adding the
  table to an existing bundle appends to its member list and leaves the
  other members byte-identical. As with the other member tables, a value
  may be deployment-relative (`./hooks/shell-guard:1`) — that still names
  a registry reference, resolved against the bundle's own directory, not a
  local path.

  **Membership declares; it never arms.** A bundle-delivered hook lands in
  the lock with the bundle as its provenance and its *own* digest-pinned
  source, then faces the identical install-time gate a directly declared
  hook faces — the feature flag first, then the consumer's workspace
  consent. There is no per-hook approval anywhere in the design: consent is
  asked once per workspace and covers the hook set that workspace declares.
  `grim add <bundle>` records the **resolved** member set, so a bundle that
  later gains a hook member drifts and re-asks rather than arming silently;
  packaging a hook inside a bundle is not a way around the gate.
- **`publish.toml` has a `[hooks]` table**, and its conventional source
  path is `hooks/<name>/` (a directory, matching the artifact shape).
  Batch publish releases hooks after `mcp` and before `bundles`.
- Everything else is ordinary: cascade tags, the immutability gate,
  `--git` provenance, and the description companion all behave exactly as
  they do for the other kinds — see
  [release-checklist.md](release-checklist.md).

## Validation Pitfalls

Every row below fails `grim build` with **exit 65**.

| Pitfall | Message you get |
|---|---|
| Handler's first token is a payload file | `runs the payload file … directly; a payload delivered through a registry is not executable` |
| `summary` / `keywords` / `repository` at top level | `unknown field 'summary', expected one of 'schema', 'name', 'description', 'hooks'` |
| `name` ≠ directory stem | `hook manifest name 'other-name' must equal the directory stem 'meta-guard'` |
| Artifact named `bin`, `dispatch.json`, `dispatch.json.lock`, `payload` or `root-key` | `hook artifact name 'bin' is reserved: 'bin' names part of grim's own hook launcher under $GRIM_HOME/hooks/ — rename the artifact` |
| Artifact name is not a plain name (`my_hook`, `MyHook`) | `hook artifact name 'my_hook' is not usable: skill name 'my_hook' must contain only lowercase letters, digits, hyphens, and periods` |
| `id` outside the charset, or over 128 bytes | `invalid hook id 'log:tool/call': expected only ASCII letters, digits, '_', '-' and '.'` |
| Two entries sharing an `id` | `duplicate hook id 'obs'` |
| Both `argv` and `command` | `declares both 'argv' and 'command'; exactly one of them is required` |
| Neither `argv` nor `command` | `declares no handler: set exactly one of 'argv' or 'command'` |
| No `event` and no `<vendor>.event` | `declares no event: set 'event' or a single '<vendor>.event' override` |
| `mutator` outside `PreToolUse` | `tier 'mutator' is not valid at event 'PostToolUse'` |
| `gatekeeper` on a verdictless event | `tier 'gatekeeper' is not valid at event 'SessionStart'` |
| `mutator` on a native-only moment | `declares tier 'mutator' on a native-only event: 'mutator' requires event = "PreToolUse"` |
| Regex or other stray characters in `matcher` | `invalid matcher '^Bash$': expected only [A-Za-z0-9_*?./-\|]` |
| `matcher` over 256 bytes | `matcher of 257 bytes exceeds the 256-byte limit` |
| `matcher = ""` | `an empty matcher is ambiguous across clients; omit 'matcher' to match every tool` |
| Override table named for something that is not a client | `key 'clod' is not a per-client override table` |
| `schema` grim does not know | `hook manifest schema version 2 is not supported (this grim understands 1)` |

## Further Reading

- `grim schema --kind hook` — the authoritative `hook.toml` shape, carrying
  grim's own doc comments for every event, tier, and field. Bind it in your
  editor before writing the manifest.
- [Client Compatibility][clients] — the enforced per-client matrix.
- [release-checklist.md](release-checklist.md) — pre-release gates and the
  exit-65 triage table.
- [../SKILL.md#hooks-ship-disarmed](../SKILL.md#hooks-ship-disarmed) — the
  gate model, stated once.

[clients]: https://grimoire.rs/clients.html
