---
title: When a hook does not arm
description: The warnings grim prints when a declared hook does not arm, what causes each one, and how to fix it. Experimental pre-1.0.
---
<!-- doc_type: troubleshooting -->

**Experimental pre-1.0**. See [Stability][stability-unstable].

A hook that did not arm shows a state other than `installed` in
`grim hook list`, with the cause in the Detail column.
`grim status --format json` carries the same cause as a `cause` token, and [Hook
arming][json-hook-arming] lists every token. The entries below cover the
warnings most readers meet first. The [hooks reference][gating] describes each
gate.

## Warning: hooks are gated {#gated}

`grim install` skips the hook with a warning, and `grim hook list` shows it as
`gated`. The Detail column names one of three causes: `feature-flag-off`,
`workspace-not-consented`, or `consent-drifted`.

This issue occurs when a policy gate holds the hook back. Either the
experimental feature is off for this scope, or this checkout has not consented
to the hooks it declares. A hook added or pulled from a new repository after you
consented counts as not consented, too.

To fix it, clear the cause the Detail column names:

- For `feature-flag-off`, run `grim config set options.experimental.hooks true`,
  then `grim install`.
- For `workspace-not-consented`, run `grim hook allow` in the project directory,
  then `grim install`. A fresh clone always starts here, because consent never
  travels with a repository.
- For `consent-drifted`, read the hooks the warning lists. Run `grim hook allow`
  if you want them, then `grim install`.

In CI, pass `--trust-hooks` to `grim install` instead of writing a record. It
answers for that one run.

## Warning: a client hosts hooks at global scope only {#global-only}

A project install warns that Codex, Copilot, or Qoder hosts hooks at global
scope only, and suggests `--global`. The hook arms for Claude and skips that
client.

This issue occurs when a project declares a hook and a client that keeps its
project-scope hooks in a tracked repository file is selected. Grim will not
write an armable file into a repository, so those clients arm only from a global
declaration.

To arm the hook for that client, turn the feature on globally with
`grim config set --global options.experimental.hooks true`. Then declare the
hook there with `grim add --global --kind hook <reference>`. Global scope is
always consented. If Claude is the only client you need at project scope, no
action is needed, and the warning is informational.

## Warning: this client's hook file exists and grim did not write it {#surface-user-owned}

`grim hook list` shows the client as `not-armed` with cause
`surface-user-owned`. The client is Codex or Copilot.

This issue occurs when grim finds a hook file at the path it manages, and the
bytes differ from what grim last wrote there. Grim owns Codex's `hooks.json` and
Copilot's `hooks/grim.json` outright. It touches either file only while it
matches what grim wrote. A file you created or edited is yours, so grim neither
overwrites nor deletes it.

To let grim manage the file, move it aside, then run `grim install`. Copy any
handlers you wrote by hand into your own hook artifact first, or they stop
running.

## Warning: the hook dispatch table would grow past its limit {#dispatch-full}

`grim install` warns that the hook dispatch table would grow past its
1048576-byte limit. Every client in the workspace shows `not-armed` with cause
`not-registered`.

This issue occurs when the machine's dispatch table at
`$GRIM_HOME/hooks/dispatch.json` has no room for this workspace's rows. One
table holds the hooks of every project on the machine, and it is capped at 1 MiB
(issue [#93][gh-93]).

To make room, run `grim uninstall hook <name>` in projects that no longer need
their hooks. Then run `grim install` again in the project that needs the space.

<!-- internal -->
[stability-unstable]: ./stability.md#unstable
[json-hook-arming]: ./json-interface.md#hook-arming
[gating]: ./hooks.md

<!-- external -->
[gh-93]: https://github.com/grimoire-rs/grimoire/issues/93
