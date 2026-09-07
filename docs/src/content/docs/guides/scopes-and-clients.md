---
title: "Choose a scope and choose your clients"
description: "Move a declaration between project and global scope, then add and drop a client."
---
<!-- doc_type: how-to -->
<!-- doc_tier: everyday -->

This guide moves one artifact between project and global scope, then adds and drops a client for your setup. It assumes `grim` is on your `PATH` and that you work inside a repository.

## Project scope or global scope

Grimoire keeps two separate sets of declarations. The **project** scope is the `grimoire.toml` that grim discovers from the current directory. The **global** scope is one config under `$GRIM_HOME` that every directory on the machine shares.

Which one you want depends on who else needs the artifact. A rule your repository depends on belongs in the project scope, because `grimoire.toml` and `grimoire.lock` are committed and every contributor gets the same set. A skill you reach for in every checkout belongs in the global scope, where nobody else has to carry it.

Every command works on the project scope and switches with `--global`. The two sets are never merged. `grim status` lists the project set and `grim status --global` lists the global one, so an artifact's scope is the invocation that lists it.

`grim context` answers the same question in one place. It prints a `scope` row, the config, lock and state paths for that scope, and the client list in force:

```sh
grim context
```

## Move an artifact between scopes

A move is an uninstall at one scope followed by an `add` at the other. Reach for [`grim remove`](../commands.md#remove) only when you want the files kept. It undeclares the artifact and leaves the installed copies on disk, so a move built on it ends with two copies of the same skill.

Before you start, the artifact has to be declared and installed at the scope you are moving it from.

1. Find the scope that holds it.

   ```sh
   grim status
   ```

2. Uninstall it there. This deletes the rendered files, drops the install record and undeclares it.

   ```sh
   grim uninstall skill code-review
   ```

3. Declare it at the other scope. A relative path is rewritten to point at the same file from the destination config's own directory. Give an absolute path instead.

   ```sh
   grim add --global $PWD/agent-config/code-review
   ```

4. Confirm the move.

   ```sh
   grim status --global
   ```

The project table prints its header row and nothing else, and the global table carries the artifact. A registry reference moves the same way: put the reference in step 3 where the path is.

## Choose which clients get your config

[`grim add`](../commands.md#add) has no `--client` flag. Clients are chosen in two other places: the `options.clients` list in your config, which every later command reads, and `--client` on [`grim install`](../commands.md#install), which applies to that one run.

1. Set the client list.

   ```sh
   grim config set options.clients claude,codex
   ```

2. Declare the artifact.

   ```sh
   grim add ./agent-config/code-review
   ```

3. Look at what the one declaration wrote.

   ```sh
   find .claude .agents -type f
   ```

Two files land from a single `add`:

```text
.claude/skills/code-review/SKILL.md
.agents/skills/code-review/SKILL.md
```

The result table prints one row per artifact and not one per file, so the disk is where the fan-out shows. `grim status --format json` lists every rendered file under `outputs`.

To bring in another client later, write the list again. `grim config set options.clients` replaces the whole list and never appends to it, so name every client you want to keep:

```sh
grim config set options.clients claude,codex,cursor
```

Then render the new client's copy. A second `grim add` is not needed, because the declaration has not changed:

```sh
grim install
```

## What detection looks at

Without an `options.clients` list, `grim install` writes to every client it detects.

At project scope a client counts as detected when its own directory sits in the workspace: `.claude` for [Claude Code](https://code.claude.com), `.cursor` for [Cursor](https://cursor.com). Claude Code also counts when the project holds a `.mcp.json`. At global scope grim looks at the user-level root instead, `~/.claude` or the directory `$CLAUDE_CONFIG_DIR` points at.

Project detection never reads your home directory. A fresh checkout with no vendor directory detects nothing at all, and `grim init` then writes no `[options]` table:

```toml
#:schema https://grimoire.rs/schemas/grimoire-config.schema.json

[skills]

[rules]
```

With nothing detected, `grim install` targets the vendor-neutral `agents` client. That is the shared pool at `.agents/skills`, which several clients read. It is only ever selected and never detected, so writing the pool changes no later run's answer.

The [client compatibility matrix](../clients.md) names every client grim knows and the kinds each one takes.

## When you drop a client

Dropping a client is the same command with a shorter list:

```sh
grim config set options.clients claude
```

Nothing on disk changes. `grim status` still prints one `installed` row, and the table has no client column to tell you otherwise:

```text
Kind   Name         Source                            Pinned  State
skill  code-review  path: ./agent-config/code-review  -       installed
```

The copy the dropped client read is still there. The file at `.agents/skills/code-review/SKILL.md` survives the config edit, because `grim install` materializes the lock and never deletes.

The JSON report is the one place the leftover surfaces. The `outputs` array stops listing a client the moment you edit the config, while `clients_extra` names the file still on disk:

```json
"clients_extra": [
  "codex"
]
```

[`grim update`](../commands.md#update) is what reaps it:

```sh
grim update --format json
```

```json
"reaped_clients": [
  "codex"
]
```

The plain table prints an `unchanged` row and says nothing about the reap, so ask for JSON when you want to watch it happen. The file is gone afterwards and an empty `.agents/skills` directory stays behind.

## See it run

<div data-cast="/casts/scopes.cast" data-cast-poster="npt:0:03"></div>
<noscript><a href="/casts/scopes.cast">Download the recording</a></noscript>

## Next steps

- [One directory every agent reads](./shared-skills.md) puts a repository's skills and rules in one committed place, which is the project scope doing its real work.
- [The client compatibility matrix](../clients.md) tells you which client takes which kind, and where each file lands.
- [Keep a hand edit across an update](./lifecycle.md) covers what install and update do when a file on disk no longer matches what grim recorded.
