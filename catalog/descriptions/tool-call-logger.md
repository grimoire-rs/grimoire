# tool-call-logger (example hook)

The reference **`observer`** example: the safe default tier, whose response
grim discards. It cannot block a tool call, cannot rewrite one, and cannot
delay one beyond its 5-second timeout.

Two entries share one payload script — a pair bound to different moments is
the common case the `[[hooks]]` array exists for:

| Entry | Event | Matcher | What it does |
|---|---|---|---|
| `log-tool-call` | `PreToolUse` | `*` (every tool) | Appends one line before each tool call |
| `log-session-start` | `SessionStart` | — | Appends one line when a session begins |

## What it writes, and where

One line per event, to `$GRIM_EXAMPLE_LOG` — or, when that is unset, to
`grim-tool-call-logger.log` under `$TMPDIR` (`/tmp` when that is unset too):

```text
PreToolUse client=claude tool=Bash hook=tool-call-logger/log-tool-call tier=observer
```

It writes nowhere else. It reads no file, opens no socket, and never
touches your repository — including its own payload directory, which grim
hashes for integrity.

## Hooks do not arm on install

Installing this changes nothing until you turn hooks on deliberately. They
are off behind the `[options.experimental] hooks` feature flag and gated
again by your workspace's own consent (`grim hook allow`); until both allow
it, `grim install` skips the hook with a warning and `grim status` names the
gate that is still closed.

```sh
grim add ghcr.io/grimoire-rs/hooks/tool-call-logger:0
```

The full walkthrough — enabling the flag, granting trust, watching a line
appear, and **disarming again** — is in
[`catalog/hooks/README.md`](https://github.com/grimoire-rs/grimoire/blob/main/catalog/hooks/README.md).

## Links

- Documentation: <https://grimoire.rs>
- Source & issues: <https://github.com/grimoire-rs/grimoire>

Published under the Apache-2.0 license.
