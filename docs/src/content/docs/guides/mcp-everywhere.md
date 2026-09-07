---
title: "One MCP server in every agent"
description: "Register one server descriptor into every client you use, then read back each client's entry."
---
<!-- doc_type: how-to -->
<!-- doc_tier: everyday -->

This guide registers one server descriptor into every client you use, then reads back each client's entry. It assumes `grim` is on your `PATH` and a project you can write to.

## Declare the server once

Every client that speaks the [Model Context Protocol](https://spec.modelcontextprotocol.io/) wants the same three facts: a name, a command, and its arguments. Each one wants them in its own file, under its own key, in its own format. Registering a server by hand across four agents means writing the same server four times and keeping four files in step.

An [MCP artifact](../mcp-servers.md) carries that descriptor once. `grim add` renders it into each client on your list, in the shape that client parses.

Before you start, note that the reference has to come from a registry. Path sources work for skills and rules, but not for this kind:

```text
path sources are not supported for mcp artifacts
```

1. Name the clients that should get the server.

   ```sh
   grim config set options.clients claude,cursor,codex,opencode
   ```

2. Declare the server.

   ```sh
   grim add ghcr.io/grimoire-rs/mcp/grim
   ```

One `added` row comes back. Four files land.

## Read each client's entry

The result table prints one row per artifact, so the fan-out is on the disk rather than in the output. Each of the four files below is yours to open and edit. Each one has its own idea of how a server is written down.

`.mcp.json` for [Claude Code](https://code.claude.com), keyed by server name under `mcpServers`:

```json
{
  "mcpServers": {
    "grim": {
      "args": [
        "mcp"
      ],
      "command": "grim"
    }
  }
}
```

`.cursor/mcp.json` for [Cursor](https://cursor.com), the same key with a transport field added:

```json
{
  "mcpServers": {
    "grim": {
      "args": [
        "mcp"
      ],
      "command": "grim",
      "type": "stdio"
    }
  }
}
```

`.codex/config.toml` for [Codex](https://developers.openai.com/codex), which is TOML rather than JSON and spells the table `mcp_servers`:

```toml
[mcp_servers.grim]
args = ["mcp"]
command = "grim"
```

`opencode.json` for [opencode](https://opencode.ai), which keys on `mcp`, folds the command and its arguments into one array, and carries an enable flag:

```json
{
  "mcp": {
    "grim": {
      "command": [
        "grim",
        "mcp"
      ],
      "enabled": true,
      "type": "local"
    }
  }
}
```

Four key names and two file formats, from one declaration. Adding a fifth agent is a longer `options.clients` list and a `grim install`, not a fifth file to write by hand.

## When you edit an entry by hand

These files are ordinary config files, and a server usually earns an extra flag sooner or later. Add one to `.mcp.json` and grim notices at once:

```sh
grim status
```

```text
Kind  Name  Source  Pinned                                                                                                State
mcp   grim  direct  ghcr.io/grimoire-rs/mcp/grim@sha256:394ab526ab21db803ecd2b5303638b07c7f64e641a1b15f61f69337e3b98384b  modified
```

That word and the rest of the state vocabulary are defined under [artifact states](../commands.md#artifact-states).

The next install refuses rather than overwriting your edit. It names both hashes and the flag that would proceed, and exits `65`. The first hash is the entry grim recorded and the second is what it found in the file. Both are elided here, because yours depend on the edit you made:

```text
mcp 'grim' (ghcr.io/grimoire-rs/mcp/grim@sha256:394ab526ab21db803ecd2b5303638b07c7f64e641a1b15f61f69337e3b98384b): installed artifact was modified locally: recorded sha256:…, found sha256:…; rerun with --force to overwrite
```

You have two ways out. Keep the edit and leave the row as it is, at the cost of a refused install every time. Or hand the file back to grim, which throws the edit away:

```sh
grim install --force
```

The durable fix is neither. An edit you want on every machine belongs in a published descriptor. Then `grim update` carries it to every client, instead of you carrying it to every file.

## Next steps

- [MCP servers](../mcp-servers.md) covers the descriptor itself: its fields, its per-client file, and what a client does with each one.
- [Choose a scope and choose your clients](./scopes-and-clients.md) shows how to install the same server user-wide with `--global`, so a new checkout starts with it.
- [Keep a hand edit across an update](./lifecycle.md) covers keeping a hand edit through an update, and taking the registration back out.
