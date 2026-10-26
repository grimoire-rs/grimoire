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

## Warning: hooks do not arm on Windows {#platform-unsupported}

`grim hook list` shows the hook as `not-armed` with cause
`platform-unsupported` for a client. With the feature flag on and a hook
installed, `grim install` warns once per client. The warning reads `hooks not
armed for <client>: grim has not verified hooks for this client on Windows, so
it was not registered`.

This issue occurs when grim runs on Windows and the client is not one grim has
verified there. Claude Code is the only one. Copilot and Codex have not yet been
observed launching a hook on Windows. Grim registers nothing for them rather
than risk a registration that denies every tool call.

Until the feature flag is on and the workspace has consented, `grim hook list`
shows that gate's cause instead. Grim declines to arm either way.

To arm the hook for an unverified client, run grim on Linux or macOS. On
Windows it arms for Claude Code. Declaring a hook is never an error, so the
declaration can stay in `grimoire.toml` for other machines.

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

## Warning: the hook dispatch table is one this grim cannot read {#dispatch-unreadable}

`grim install` warns `dispatch table write failed; nothing armed`. The error
names `$GRIM_HOME/hooks/dispatch.json` and says it is "not a table this grim can
update". Every client in the workspace shows `not-armed` with cause
`not-registered`.

This issue occurs when the machine's dispatch table has a schema this grim does
not know, does not parse, or is over its 1 MiB cap. The error names which
in parentheses: `UnknownSchema`, `Unparsable` or `Oversize`. One table holds the
hooks of every project on the machine, so grim leaves it untouched. Overwriting
it would erase rows it cannot read.

If another grim version wrote the table, upgrade grim. Otherwise move
`dispatch.json` aside and run `grim install` again in each project that uses
hooks. Each project rebuilds only its own rows.

## Warning: grim could not place its launcher {#launcher-not-placed}

`grim hook list` shows the client as `not-armed` with cause
`launcher-unwritable` or `launcher-path-occupied`. The launcher is the small
program every registration runs, at `$GRIM_HOME/hooks/grim-hook`
(`grim-hook.exe` on Windows). The `launcher` object in
`grim hook list --format json` reports its state.

This issue occurs when grim cannot write the launcher. For
`launcher-unwritable`, `$GRIM_HOME/hooks` or a file already at the launcher's
path is not writable by you. For `launcher-path-occupied`, something other
than a file sits at that path, such as a directory. Grim never removes it.

To fix it, make `$GRIM_HOME/hooks` and any existing file there writable. If a
directory occupies the path, move it aside. Then run `grim install` again.

## Warning: the launcher path cannot be registered {#launcher-path}

`grim hook list` shows the client as `not-armed` with cause
`launcher-path-control-character` or `launcher-path-unquotable`.

This issue occurs when the path to the launcher cannot be written into a
client's hook configuration without ambiguity. A newline or another control
character in `$GRIM_HOME` makes the first cause, for every client. The second
is a path that is not valid UTF-8. It also covers, on Windows for a client that
registers a `cmd` command line, a character that command line cannot quote,
such as a space.

To fix it, point `GRIM_HOME` at a plainer path, then run `grim install` again.

## Warning: GRIM_HOME is shared between hosts {#grim-home-shared}

`grim hook list` shows every client as `not-armed` with cause
`grim-home-shared`.

This issue occurs when `GRIM_HOME` is not a local path under
`%USERPROFILE%` or `%LOCALAPPDATA%`, for example a UNC path on a network share.
Grim makes this check on Windows only. The launcher is a native program built
for one host, so each host sharing the home would overwrite the other's copy.

To fix it, give each host its own `GRIM_HOME` on a local disk, then run
`grim install` again.

## Warning: a hook is bound under a name grim keeps {#binding-name-reserved}

`grim hook list` shows the hook as `not-armed` with cause
`binding-name-reserved`.

This issue occurs when a hook is bound under a name grim writes under
`$GRIM_HOME/hooks`: `bin`, `consent`, `dispatch.json`, `dispatch.json.lock`,
`grim-hook`, `grim-hook.exe`, `payload` or `root-key`. It also occurs for a
binding name that ends in `.exe`, `.com`, `.bat` or `.cmd`, which Windows could
run in place of the launcher. Grim keeps all of these names.

To fix it, rename the binding in `grimoire.toml`, for example
`grim add --kind hook --name <name> <reference>`, then run `grim install`.

## Warning: Codex no longer trusts the hook command {#codex-retrust}

`grim install` warns that `codex no longer trusts the hook command grim
registered for <hook>`. `grim hook list` shows the Codex row as `untrusted`
with cause `client-trust-pending`.

This issue occurs when grim rewrites the command it registered for a gatekeeper
hook in Codex's `hooks.json`. Codex keys its stored approval on the exact
command text, so a changed command is a new, unapproved hook. Codex skips it
silently until you approve it. This happens once after upgrading to the native
launcher, and again only if the command changes, for example when you move
`GRIM_HOME`.

To fix it, open Codex's `/hooks` screen and approve the hook again.

## Warning: hooks stop firing or error after upgrading grim {#after-upgrade}

A hook that fired before an upgrade does nothing. Or the client reports an
error on every tool call, in a `grim-hook` line. Exit `79` means the
`dispatch.json` beside the launcher is missing, and exit `65` means it is
unreadable. See [launcher exit codes][launcher-exit] for every status.

This issue occurs when a registration outlives what it pointed at. One case is a
workspace you have not revisited since the upgrade. It still carries a
registration from the shell-script release, which pointed at `bin/grim-hook` and
at a `grim hook run` command, neither of which exists in the launcher release. That registration exits `0`
without running anything, so it never blocks a tool call.

The other case is a current registration whose launcher or table is gone, for
example after you deleted `$GRIM_HOME/hooks`.

To fix both, re-run `grim install` in each workspace and each scope that uses
hooks after you upgrade grim. It writes the launcher and the table, deletes the
old script, and rewrites that scope's registrations. Hooks are experimental, so
this change is outside the compatibility promise. See
[Stability][stability-unstable].

## Warning: the launcher exits 74, 1 or 64 {#launcher-failure}

The client reports an error on a tool call, in a `grim-hook:` line that the
launcher wrote to stderr. The status is `74`, `1` or `64`. See [launcher exit
codes][launcher-exit] for the full table.

This issue occurs when the launcher hits a failure, and the cause depends on
the status:

- Exit `74` is an I/O failure. The launcher could not find its own path, or read
  the table or the client's input. It also fires when a gatekeeper could not run
  because grim could not write its audit record, or its `payload = "file"`
  envelope, beside the dispatch table.
- Exit `1` is any other internal failure. The usual one is a gatekeeper whose
  envelope could not be built.
- Exit `64` means the registration does not match the launcher's grammar,
  `grim-hook <client> <event> <root>`. A hand-edited client hook file causes it.

To fix exit `74`, make `$GRIM_HOME/hooks` writable and check the disk has free
space. Then run `grim install` again.

To fix exit `1`, set `GRIM_LOG=debug` in the client's environment and read the
`grim-hook:` line for the cause. If it names the table or the launcher, run
`grim install`.

To fix exit `64`, run `grim install`, which rewrites the registration. Do not
edit it by hand.

## Warning: Claude Code blocks tool calls on Windows {#claude-powershell}

Every Claude Code tool call is blocked, and the error names a `grim-hook`
command that ran through PowerShell.

This issue occurs when Claude Code runs the hook through PowerShell because Git
Bash is not installed, and the launcher exits non-zero. Claude Code's
documentation treats a failing PowerShell hook as a block. A hook that runs
through `sh` or Git Bash only shows a non-blocking error. Grim does not override
a client's policy. No run on Windows has confirmed this behavior yet.

To fix it, find the failing status in [launcher exit codes][launcher-exit] and
clear its cause. Most often `grim install` fixes it by writing the missing
launcher or table. Installing Git for Windows also moves Claude Code onto the
non-blocking path.

<!-- internal -->
[stability-unstable]: ./stability.md#unstable
[json-hook-arming]: ./json-interface.md#hook-arming
[gating]: ./hooks.md
[launcher-exit]: ./hooks.md#launcher-exit

<!-- external -->
[gh-93]: https://github.com/grimoire-rs/grimoire/issues/93
