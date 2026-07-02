# MCP Server Spec

You loaded this file because you are authoring or fixing a grim MCP
server descriptor — a `.toml` file describing one Model Context Protocol
server — for `grim build --kind mcp` or `grim release --kind mcp`.

Contents: [File Shape](#file-shape) · [Top-Level Keys](#top-level-keys) ·
[The Server Table](#the-server-table) · [Env References](#env-references) ·
[What Each Client Receives](#what-each-client-receives) ·
[Example](#example) · [Validation Pitfalls](#validation-pitfalls)

## File Shape

An MCP server descriptor is one `.toml` file named by its file stem under
the standard name rules, with catalog metadata at the top level and a
single `[server]` table. It never materializes a file at install time —
grim registers a vendor-native entry in each client's own MCP config
(Claude's `.mcp.json` / `~/.claude.json`, OpenCode's `opencode.json`,
the VS Code / Copilot surfaces) and removes exactly that entry on
uninstall.

**`--kind mcp` is mandatory.** A `.toml` is bundle-shaped by default;
grim errors with a `--kind mcp` hint when it sees a `[server]` table on
the bundle path.

## Top-Level Keys

Same location rule as bundles — top level, not nested:

| Key | Notes |
|---|---|
| `description` | **Required**, non-empty. Becomes the OCI description annotation. |
| `summary` | Optional short catalog blurb. |
| `keywords` | Optional, one comma-separated string. |
| `repository` | Optional, must be `https://` (65 otherwise). |
| `deprecated` | Optional deprecation notice. |

## The Server Table

`transport` picks the shape; mixing shapes fails validation (65):

| Transport | Required | Allowed | Forbidden |
|---|---|---|---|
| `stdio` | `command` | `args`, `env` | `url`, `headers` |
| `http` / `sse` | `url` (http(s) scheme) | `headers` | `command`, `args`, `env` |

`env` keys must match `[A-Za-z_][A-Za-z0-9_]*`.

## Env References

Values may reference host environment variables with the canonical
`${VAR}` form — never a literal secret. Grim translates the reference
per client at install time (`{env:VAR}` for OpenCode, `${env:VAR}` for
the VS Code config; Claude reads `${VAR}` natively).

- `${VAR:-default}` is **rejected** (grim 0.7.x) — only Claude supports
  defaults natively, so a default would behave differently per client.
- Copilot CLI's global `mcp-config.json` supports no substitution at
  all: a descriptor that uses `${VAR}` skips that client with a warning
  rather than ever writing a secret (or a broken literal) to disk.
- A bare `$VAR` (no braces) is a literal, not a reference.

## What Each Client Receives

Grim renders the vendor's own schema — confirm the authoritative matrix
on the docs site ([MCP Server Artifacts][mcp-docs]). Highlights: OpenCode
gets `command` as ONE array (`["grim", "mcp"]`) under `type: "local"`
with env under `environment`; the VS Code config uses `type: "stdio"`;
Copilot CLI's global entry gains `tools: ["*"]`. Only the managed entry
is ever touched — user keys, formatting, and comments in the config file
survive, and grim's drift check is semantic (reordering the file is not
a modification; editing the entry's values is).

## Example

```toml
description = "Grimoire catalog search and install status over MCP."
summary = "grim as an MCP server"
keywords = "grimoire,mcp,catalog"
repository = "https://github.com/grimoire-rs/grimoire"

[server]
transport = "stdio"
command = "grim"
args = ["mcp"]
env = { GRIM_HOME = "${GRIM_HOME}" }
```

## Validation Pitfalls

- Forgetting `--kind mcp`: the file hits the bundle parser (grim hints).
- `description` missing or whitespace-only → 65.
- `url` on a stdio server, or `command`/`args`/`env` on a remote one → 65.
- `${VAR:-fallback}`, `${1BAD}`, `${UNCLOSED` anywhere in a string value → 65.
- Not a bundle member: MCP descriptors cannot be listed in a bundle
  (grim 0.7.x) — declare them directly.

[mcp-docs]: https://grimoire.rs/mcp-servers.html
