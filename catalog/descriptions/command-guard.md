# command-guard (example hook)

A **`gatekeeper`** demonstration: it shows how a refusal reaches the client
as a JSON verdict. Bound to `PreToolUse` with the matcher `Bash`, it refuses
a tool call whose text contains the literal `rm -rf /` and answers with no
opinion otherwise.

> **This is not a security control.** The gatekeeper tier is not a security
> boundary in grim's design, and this example's own check is a substring
> match over the raw envelope: `rm  -rf /`, `rm -fr /`, and anything the
> agent assembles at runtime all pass straight through. Copy it to learn the
> response shape — never to enforce a policy.

## What it does at runtime

Reads the envelope on stdin, writes one JSON object on stdout, exits **0**
on every path — including the refusal. A verdict travels as a document, not
as an exit code: some clients read a non-zero hook exit as "deny", so an
internal error would silently become a refusal.

```json
{"decision":"deny","reason":"command-guard (a demonstration hook) refused a command containing the literal rm -rf /"}
```

grim projects that onto the invoking client's own response shape, so the
payload never learns which client asked.

The permissive answer is `{}` — no opinion — deliberately **not**
`{"decision":"allow"}`: on Claude and Copilot an explicit allow suppresses
the client's own approval prompt, which grants privilege rather than
declining to object.

It changes nothing on disk. No file is written, no network call is made.

## Hooks do not arm on install

Installing this changes nothing until you turn hooks on deliberately. They
are off behind the `[options.experimental] hooks` feature flag and gated
again by your workspace's own consent (`grim hook allow`); until both allow
it, `grim install` skips the hook with a warning and `grim status` names the
gate that is still closed.

```sh
grim add ghcr.io/grimoire-rs/hooks/command-guard:0
```

The full walkthrough — enabling the flag, granting trust, triggering the
refusal, and **disarming again** — is in
[`catalog/hooks/README.md`](https://github.com/grimoire-rs/grimoire/blob/main/catalog/hooks/README.md).

## Links

- Documentation: <https://grimoire.rs>
- Source & issues: <https://github.com/grimoire-rs/grimoire>

Published under the Apache-2.0 license.
