# Example hooks

Two first-party `hook` packages, published from this directory like every
other catalog package (`task catalog:verify` builds them; `publish.toml`
ships them). They exist to be **read and run**, so the format has a worked
example rather than only a schema.

| Package | Tier | Events | What it does |
|---|---|---|---|
| [`tool-call-logger`](./tool-call-logger) | `observer` | `PreToolUse`, `SessionStart` | Appends one line per event to a log file outside the repository |
| [`command-guard`](./command-guard) | `gatekeeper` | `PreToolUse` (matcher `Bash`) | Refuses a command containing the literal `rm -rf /`, as a JSON verdict |

**Start with `tool-call-logger`.** It is the safe default tier: an
observer's response is discarded, so it cannot block, alter, or redirect
anything. `command-guard` is a *demonstration* of how a verdict reaches the
client — the gatekeeper tier is **not a security boundary** in this design,
and that example's own check is a substring match that trivial rewording
evades. Neither is in the `grim-essentials` bundle: an example should be
installed on purpose.

**No `mutator` example ships here, deliberately.** Mutator is the
highest-risk tier — a rewrite the model never asked for — and the example a
reader would most want (rewriting a shell command) is exactly the one grim
refuses per client, because a tool whose input is a shell-command string is
`Declined` for mutators. A first-party example is the thing readers copy;
this one would teach the pattern grim declines to honour.

## What these show about the format

- **`argv`, not `command`.** A payload delivered through an OCI registry
  arrives without an exec bit, so `command = "./guard.sh"` is refused at
  `grim build`. The interpreter form — `argv = ["sh", "guard.sh"]` — is what
  works, and it involves no shell, so no quoting question exists.
- **POSIX `sh`, not bash.** A payload runs under whatever the client's
  environment provides.
- **stdin in, stdout out, exit 0.** grim writes one JSON envelope to the
  payload's stdin; the payload writes one canonical JSON response to stdout
  and exits 0 — *including when it refuses*. A verdict travels as a
  document, never as an exit code, because some clients read a non-zero hook
  exit as "deny".
- **`{}` is the permissive answer, not `{"decision":"allow"}`.** On Claude
  and Copilot an explicit allow *suppresses the client's own approval
  prompt*, so it grants privilege rather than declining to object.
- **Two entries can share one payload tree.** `tool-call-logger` binds the
  same script to two moments; that is what the `[[hooks]]` array is for.
- **Nothing is written into the repository.** Payloads live under
  `$GRIM_HOME`, never in the workspace.

## Manual test — a full walkthrough

Runnable end to end, from a clean `$GRIM_HOME` through arming, firing, and
disarming again. Every command below was executed against a local
`registry:2` while writing this; the transcript is in `.agents/wp-v-report.md`.

### 0. A clean, disposable environment

Nothing here touches your real config: `$GRIM_HOME` and `$HOME` are
redirected, and the demo project is a fresh directory.

```sh
export DEMO=/tmp/grim-hook-demo
mkdir -p "$DEMO/grim-home" "$DEMO/home" "$DEMO/project/.claude"
export GRIM_HOME="$DEMO/grim-home"
export HOME="$DEMO/home"          # so global config lands in the sandbox too
cd "$DEMO/project"
```

`.claude/` is created because grim installs hooks for the clients it detects,
and Claude at project scope is the only client that arms there.

### 1. Choose where the examples come from

```sh
# Consumers: the published packages.
export REF_LOGGER=ghcr.io/grimoire-rs/hooks/tool-call-logger:0
export REF_GUARD=ghcr.io/grimoire-rs/hooks/command-guard:0
```

Maintainers testing an unreleased change push to a local registry instead —
this is the variant that was actually executed:

```sh
# From the repo root, with a registry:2 on localhost:5000.
grim release catalog/hooks/tool-call-logger localhost:5000/grim-examples/hooks/tool-call-logger:0
grim release catalog/hooks/command-guard     localhost:5000/grim-examples/hooks/command-guard:0
export REF_LOGGER=localhost:5000/grim-examples/hooks/tool-call-logger:0
export REF_GUARD=localhost:5000/grim-examples/hooks/command-guard:0
```

### 2. Declare them — and watch nothing arm

```sh
grim init
grim add --kind hook "$REF_LOGGER"
grim add --kind hook "$REF_GUARD"
```

**Expect:** each `add` succeeds (exit 0) and prints a `WARN` naming the
remedy —

```text
WARN hook 'tool-call-logger' not installed: hooks are gated;
     enable them with `grim config set options.experimental.hooks true`
```

The hooks are now declared and pinned in `grimoire.lock`, and **nothing is
armed**: no dispatch table, no launcher, no client registration. Confirm:

```sh
grim status
```

**Expect:** both rows read `State: gated`, `Note: claude: feature-flag-off`.

> `grim status` is the arming report used throughout this walkthrough, one row
> per artifact. `grim hook list` covers the same ground **per `[[hooks]]`
> entry** — one row per entry per affected client, with the tier and the events
> — so reach for it when an artifact declares several entries and you want to
> know which of them armed.

### 3. Turn hooks on, and consent this workspace

Two independent gates, on purpose.

```sh
# Gate 1 — the feature flag (per project).
grim config set options.experimental.hooks true

# Gate 2 — this workspace's consent to arming what it declares.
grim hook allow
```

`grim hook allow` is idempotent here, because **`grim add` already recorded
consent in step 2** — typing a reference is the declaration gesture, and it
is one of only three things that write a record. Run it explicitly when the
declaration did not come from you: a `grimoire.toml` you cloned, one a
teammate committed, or one you hand-edited. That is the case the gate exists
for — cloning a repository is not consenting to it.

The record is machine-local, one file per workspace under
`$GRIM_HOME/hooks/consent/`, and it names this checkout by absolute path.
Consent in this sandbox arms nothing in any other directory. For a one-off
run without writing anything, `grim install --trust-hooks` arms for that
invocation only and `--no-trust-hooks` refuses it; either flag outranks the
record, in the direction you typed, and neither writes one.

### 4. Install — this is the step that arms

```sh
grim install
```

**Expect:** two rows, `Status: updated`, each `Target` pointing **inside
`$GRIM_HOME`** — `.../grim-home/hooks/payload/<workspace-key>/<name>` —
never into your project.

Check what got armed:

```sh
python3 -c "
import json, os
d = json.load(open(os.environ['GRIM_HOME'] + '/hooks/dispatch.json'))
for token, root in d['roots'].items():
    print('root token:', token, '->', root['root'])
    for r in root['hooks']:
        print(' ', r['artifact'], r['id'], r['client'], r['event'], r['tier'], r['matcher'])
"
```

**Expect:** three rows — `command-guard/refuse-recursive-root-delete`
(gatekeeper, PreToolUse, `Bash`), `tool-call-logger/log-session-start`
(observer, SessionStart), `tool-call-logger/log-tool-call` (observer,
PreToolUse, `*`) — all for client `claude`, under one opaque root token.

`.claude/settings.local.json` now carries grim-owned handler elements marked
`"com.grimoire.managed": "hook-dispatcher"`. Their `command` holds the
**absolute** launcher path (never `$GRIM_HOME`) and that opaque root token.

### 5. Fire them

Point the logger somewhere you can watch, then run the very command your
client will run, with a payload of the shape the client sends:

```sh
export GRIM_EXAMPLE_LOG="$DEMO/hooks.log"

CMD=$(python3 -c "
import json
s = json.load(open('.claude/settings.local.json'))
print(next(g['hooks'][0]['command'] for g in s['hooks']['PreToolUse'] if g['matcher'] == 'Bash'))
")

# (a) a harmless command
printf '%s' '{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"ls -la"},"cwd":"/repo","session_id":"demo-1"}' | sh -c "$CMD"

# (b) the destructive literal the guard refuses
printf '%s' '{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"rm -rf / --no-preserve-root"},"cwd":"/repo","session_id":"demo-1"}' | sh -c "$CMD"
```

**Expect (a):** no output, exit 0. Nobody had an opinion.

**Expect (b):** exit **0** — still zero — and one JSON document on stdout,
already in Claude's own shape:

```json
{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"command-guard (a demonstration hook) refused a command containing the literal rm -rf /"}}
```

**Expect (both):** the observer logged a line each time, because `*` matches
`Bash` too:

```sh
cat "$GRIM_EXAMPLE_LOG"
```

```text
PreToolUse client=claude tool=Bash hook=tool-call-logger/log-tool-call tier=observer
PreToolUse client=claude tool=Bash hook=tool-call-logger/log-tool-call tier=observer
```

Every invocation also leaves an audit record — verdict, tier, digest,
correlation id — at `$GRIM_HOME/hooks/hook_audit.jsonl`:

```sh
tail -2 "$GRIM_HOME/hooks/hook_audit.jsonl"
```

To see `SessionStart` fire, run the same thing with that group's command:

```sh
CMD=$(python3 -c "
import json
s = json.load(open('.claude/settings.local.json'))
print(s['hooks']['SessionStart'][0]['hooks'][0]['command'])
")
printf '%s' '{"hook_event_name":"SessionStart","cwd":"/repo","session_id":"demo-1"}' | sh -c "$CMD"
tail -1 "$GRIM_EXAMPLE_LOG"
```

**Expect:** `SessionStart client=claude tool=none hook=tool-call-logger/log-session-start tier=observer`.

### 6. Disarm — do not skip this

Arming something and walking away is a trap. `grim uninstall` is the full
inverse: it reaps the dispatch row, the client registration, and the payload.

```sh
grim uninstall hook command-guard
grim uninstall hook tool-call-logger
```

**Expect:** both report `Status: uninstalled`, and all three surfaces are
empty —

```sh
python3 -c "
import json, os
d = json.load(open(os.environ['GRIM_HOME'] + '/hooks/dispatch.json'))
print('rows:', [r for root in d['roots'].values() for r in root['hooks']])
"
cat .claude/settings.local.json
ls "$GRIM_HOME/hooks/payload/"*/ 2>/dev/null || echo '(no payloads left)'
```

`rows: []`, a `settings.local.json` back to `{}`, and no payload directories.

Withdraw this workspace's consent as well:

```sh
grim hook revoke
```

`revoke` is idempotent — running it in a workspace that never consented
still exits `0` — and, like the feature flag, it does not disarm by itself:
the next `grim install` is what converges.

To turn the feature flag back off:

```sh
grim config set options.experimental.hooks false   # or: grim config unset …
grim install                                        # convergence is what disarms
```

The config write warns that it has not disarmed anything by itself, and that
warning is accurate: the dispatch table, the launcher and the client
registration are all still on disk until `grim install` converges. Check with
`grim status` — the rows should read `gated` and the registration should be
gone.

Finally, throw the sandbox away:

```sh
rm -rf "$DEMO"
```

## Two gaps this walkthrough found, both since fixed

Recorded because the fixes are what the steps above now rely on, and because
both were found by writing the walkthrough rather than by reading the code:

- **`grim hook list` returned no rows**, whatever was armed — an unconditional
  empty report left behind when the surrounding feature landed. Now populated
  from the resolved scope, sharing `grim status`'s verdict derivation so the two
  commands cannot disagree about one hook.
- **The feature flag had no CLI route back to `false`.** Both `config set …
  false` and `config unset` were refused, and the refusal's own message named
  `grim install`, which converges but cannot clear a flag — so hand-editing
  `grimoire.toml` was the only way out. The write is now permitted and warns
  that convergence still has to run.

## Maintaining these packages

```sh
task catalog:verify        # grim build every catalog package, hooks included
```

`grim build catalog/hooks/<name>` alone also works — path inference tests
for `hook.toml` before `SKILL.md`, so no `--kind` flag is needed.

Acceptance coverage lives in `test/tests/test_example_hooks.py`: it
releases these exact directories to a local registry, arms them, fires them,
and asserts the documented side effects.
