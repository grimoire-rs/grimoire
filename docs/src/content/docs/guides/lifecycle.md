---
title: Keep a hand edit across an update
description: Install, update, inspect and uninstall an artifact while keeping or deliberately discarding a hand edit.
---
<!-- doc_type: how-to -->
<!-- doc_tier: everyday -->

By the end of this guide you have installed an artifact and edited one of
its files by hand. You have then updated it, chosen to keep or drop that
edit on purpose, and uninstalled the artifact. You need a project with a
`grimoire.toml`, which `grim init` writes, and one artifact reference you
can reach.

## Install and see what grim recorded

1. Declare the artifact and install it in one gesture.

   ```sh
   grim add ghcr.io/acme/code-review:1
   ```

   `grim add` writes the artifact into `grimoire.toml`, pins it in
   `grimoire.lock`, and installs it into the clients it detects.

2. Read back what grim recorded.

   ```sh
   grim status
   ```

   The `State` column carries one word per artifact. Each of those words is
   defined in [artifact states](../commands.md#artifact-states), and no
   other page repeats them.

## Check what changed upstream

`grim status --check` goes to the network and reports what moved behind your
declarations. It loads the catalog once, then re-resolves the tag behind every
registry-locked artifact at bounded concurrency.

```sh
grim status --check --format json
```

It fills in three fields per artifact. `update_available` answers whether
`grim update` would move that pin, `deprecated` carries any notice the
publisher wrote, and `replaced_by` names a successor.

Two things about the command surprise people. Its plain table is
byte-identical to the plain `grim status` table, because those three fields
reach you only under `--format json`. And `grim status` reports rather than
judges: no state it finds changes its exit code, `--check` included, so it
cannot be a CI gate on its own. Only a broken read fails it, such as a corrupt
lock, which exits `78`.

grim answers `update_available` by re-resolving the reference you declared.
A newer release your declaration does not point at is not an available
update, which is the reference ladder at work.

## Edit a file by hand

1. Append a line to the file grim installed.

   ```sh
   echo 'Also check the tests.' >> .claude/skills/code-review/SKILL.md
   ```

2. Ask grim what it makes of the change.

   ```sh
   grim status
   ```

   The row for that artifact reads `modified`.

## What install and update do with your edit

Both commands compare the bytes on disk against the hash grim recorded. A
hand edit fails that comparison, and both refuse rather than overwrite it.

`grim install` refuses with exit `65` and prints no table. One line reaches
stderr, naming the hash it recorded and the hash it found, and saying that
`--force` would overwrite. Your file is untouched.

`grim update` refuses with exit `65` as well. It prints an `ERROR` line
saying the update was refused, and warning that `--force` would also
authorize update's prune and reap deletions. Unlike install, it still
prints its report table beside that error.

There is a trap here, and it is the reason this page exists. A refused
`grim update` has already re-resolved your declaration and rolled the lock
pin forward. The refusal stops the write to your file, not the write to the
lock. So the lock names a release your file has never held, and the next
`grim install --force` writes that release over your edit.

Pruning and client-reaping happen on update only. `grim install` never
deletes anything.

## Keep the edit

Keeping the edit takes no flag. Leaving `--force` off is the whole
mechanism.

1. Run the update and read the refusal.

   ```sh
   grim update
   ```

   The command exits `65`, and the file you edited is untouched.

2. Confirm the line you added survived.

   ```sh
   tail -1 .claude/skills/code-review/SKILL.md
   ```

Your edit survives, but the lock pin did not stand still. Decide which
content you want before the next install, because a forced install writes
whatever the lock names.

## Discard the edit

1. Overwrite your edit with the content the lock pins.

   ```sh
   grim install --force
   ```

   grim prints a `Kind | Name | Target | Status` table reading `updated`,
   and exits `0`.

One warning applies here. After a refused `grim update` the lock already
names a different release. The forced install writes that release, not the
version you started from.

## Remove or uninstall

Both commands undeclare the artifact. They differ on whether the files
survive, so pick the one that matches what you want to keep.

### Keep the files and stop managing them

```sh
grim remove skill code-review
```

`grim remove` drops the artifact from `grimoire.toml` and from the lock,
prints a `Kind | Name | Status` table reading `removed`, and exits `0`. The
installed files stay on disk. That is how you keep an edited file for good.

### Delete the files too

```sh
grim uninstall skill code-review
```

`grim uninstall` deletes the files, drops the install record, and
undeclares the artifact.

Both take a kind and a name. The kind is `skill`, `rule`, `agent`, `bundle`
or `mcp`, and `grim uninstall` accepts every one of those but `bundle`.

## See it run

<div data-cast="/casts/lifecycle.cast" data-cast-poster="npt:0:03"></div>
<noscript><a href="/casts/lifecycle.cast">Download the recording</a></noscript>

## Next steps

Read [Check an artifact before you install it](./inspect.md) or
[Choose how far a reference is allowed to move](./versioning.md).
