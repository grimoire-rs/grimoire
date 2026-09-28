---
title: Hand a team a plugin without grim
description: Package a shared skill, agent, and MCP set as a Claude Code or Agent Plugins plugin and hand it to a teammate directly — no grim install required on their end.
---
<!-- doc_type: how-to -->
<!-- doc_tier: integration -->

Goal: turn a set of already-published skills, agents, and MCP servers
into one plugin file, using [`grim export
plugin`](../commands.md#export-plugin). Hand that file to a teammate who
has never installed grim.

## Before you start

You need `grim` and a registry your artifacts are already published to.
[Installation](../installation.md) covers putting the binary in place;
[Publishing](../publishing.md) covers pushing a skill, agent, or MCP
descriptor there first. Your teammate needs neither. They only need
[Claude Code][claude-code] or another client, and the file this guide
produces.

## Declare the plugin

Create a `marketplace.toml` naming the artifacts this plugin bundles:

```toml
[plugins.team]
include = [
  "ghcr.io/acme/skills/code-review:1",
  "ghcr.io/acme/agents/reviewer:1",
  "ghcr.io/acme/mcp/postgres:1",
  "ghcr.io/acme/rules/team-style:1",
]
description = "The acme platform team's review workflow"
```

This file is separate from `grimoire.toml`. Declaring a plugin here does
not install anything into your own project, and a project with no
`marketplace.toml` behaves exactly as before. The [full reference for this
file](../configuration.md#marketplace-toml) covers `rename` and per-plugin
`version`, which this guide skips.

## Export it

```sh
grim export plugin --plugin team --client claude --zip -o dist
```

```text
Plugin  Client  Version             Path                                     Omitted
team    claude  0.0.0+c2cfaefc6f09  /home/alex/acme-team/dist/team.claude.zip  1
```

`--client claude` targets [Claude Code][claude-code]'s plugin shape (a
`.claude-plugin/plugin.json` manifest). Swap it for `--client codex` or
`--client cursor` instead, to target the [Agent Plugins][agent-plugins]
specification. That is the shape [Copilot][copilot], [Codex][codex], and
[Cursor][cursor] share — see the [client families
table](../commands.md#export-plugin-families). `--zip` writes one archive
instead of a directory, matching what an upload flow expects.
This same run writes `marketplace.lock` beside `marketplace.toml`,
pinning `team`'s digests the way `grimoire.lock` pins your own project.

The `Omitted` column counts members no `claude` client can host
faithfully — here, a rule dropped for lacking a plugin-side scoping
surface. The full list, with each member's reason, is in the `--format
json` report. See [the export report shape](../json-interface.md#shapes-items)
and [what each client admits](../commands.md#export-plugin-admission).

## Hand it to your teammate

Send `dist/team.claude.zip` however you already share files — chat, a
shared drive, a repository release asset. In Claude Code or the Claude
desktop app, your teammate opens **Customize > Plugins > Add > Upload
plugin** and selects the file. [Upload a plugin][claude-plugin-upload]
covers this from their side. Nothing else installs: no registry
credential, no `grim`, no marketplace repository to add.

## What does not travel

### A relative stdio command loses its anchor {#relative-commands}

A plugin has no project root. A relative `command` such as
`./bin/postgres-mcp` works when grim installs it into your project, but
not inside a plugin. Use an absolute path, a command on `PATH`, or a
plugin placeholder in `args` that points at the plugin's own folder:

```toml
[server]
transport = "stdio"
command = "node"
args = ["${CLAUDE_PLUGIN_ROOT}/bin/srv.js"]
```

### Plugin placeholders per client {#plugin-placeholders}

Write [`${CLAUDE_PLUGIN_ROOT}`][claude-plugin-root] or the spec's
`${PLUGIN_ROOT}`. The data folder is `${CLAUDE_PLUGIN_DATA}` or
`${PLUGIN_DATA}`. The export writes each client's own names:

| Export for | Names written | Fields where they work |
|---|---|---|
| `claude` | `${CLAUDE_PLUGIN_ROOT}`, `${CLAUDE_PLUGIN_DATA}` | `command`, `args`, `env`, `url`, headers. Claude has no `cwd`. |
| `codex`, `cursor`, `copilot`, `agents` | `${PLUGIN_ROOT}`, `${PLUGIN_DATA}` | `args`, `env` values, `cwd` |
| `droid`, `junie`, `openclaw` | Your spelling, unchanged | See the [admission table](../commands.md#export-plugin-admission) |

A placeholder in `args` or `env` values works for `claude` and every
Agent Plugins client. Only an export renames it. `grim install` writes
the descriptor as is.

### Other environment variables {#plugin-env-vars}

A reference like `${TOKEN}` reads the user's environment:

- `claude` expands it in `command`, `args`, `env`, `url` and headers.
- `codex`, `cursor`, `copilot` and `agents` may not expand it. In
  `args`, `env` values or `cwd`, grim exports the server and warns you.
- For those four clients, any `${…}` in `command`, `url`, an env key or a
  header leaves the server out as `not-representable`.

### The same version can mean different bytes {#version-drift}

`plugin.json`'s `version` is not a hash of every byte in the plugin, only
of each member's own content. Editing `marketplace.toml`'s `description`
can change the plugin's bytes without moving `version` at all. So can
re-exporting after a grim release changes how a client renders a skill
(see [export member bytes](../stability.md#unstable)). That is fine for a
private handoff where you re-export and re-send the file.

Publishing this plugin somewhere versions are expected to be immutable is
different. Bump `--version` (or `marketplace.toml`'s `version`) by hand
whenever you re-export on purpose. That is the same discipline an
[immutable release tag](../publishing.md#dry-runs-and-overwrites) asks of
a registry release.

## Keep it current

Re-running the export command above does **not**, by itself, pick up a
tag that moved on the registry. A declared export only re-resolves the
plugins whose declaration or pins have gone stale. A plugin that is
already fresh is carried over byte-for-byte from `marketplace.lock`
instead. New digests arrive only two ways: editing `marketplace.toml`'s
`include` list (which marks that plugin stale), or rolling the lock
forward directly:

```sh
grim update --marketplace marketplace.toml team
```

This updates `marketplace.lock` and installs nothing. See
[`--marketplace`](../commands.md#update-marketplace) for the full selector
grammar. The *next* `grim export plugin` then renders whatever that run
just wrote.

## Share the project you already run {#from-lock}

When the plugin should be exactly what your project installs, skip the
separate manifest. Name the plugin once in `grimoire.toml`, then export
the lock:

```toml
[plugin]
name = "team"
description = "The acme platform team's shared skills"
```

```sh
grim export plugin --project --client claude --zip -o dist
```

The export renders the pins in `grimoire.lock` without re-resolving any
tag, so the plugin matches what `grim install` gave your team. After
`grim update`, export again to ship the new pins. A marketplace can list
the same project with `project = "."` in place of `include`; see
[project plugins](../configuration.md#marketplace-project).

<!-- external -->
[claude-code]: https://docs.anthropic.com/en/docs/claude-code
[agent-plugins]: https://agent-plugins.org/
[claude-plugin-upload]: https://claude.com/docs/plugins/overview
[claude-plugin-root]: https://code.claude.com/docs/en/plugins/components#path-variables-and-persistent-data
[copilot]: https://github.com/features/copilot
[codex]: https://developers.openai.com/codex
[cursor]: https://cursor.com
