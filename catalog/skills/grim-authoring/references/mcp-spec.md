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
the VS Code / Copilot surfaces, and Codex's `config.toml` under
`[mcp_servers.<name>]`) and removes exactly that entry on uninstall.
Codex's config is TOML, not JSON — grim splices it span-preserving the
same way, so surrounding user keys and comments survive.

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
| `license` | Optional SPDX-style id (e.g. `Apache-2.0`); becomes the OCI license annotation. |
| `repository` | Optional, must be `https://` (65 otherwise). |
| `deprecated` | Optional deprecation notice. |

## The Server Table

`transport` picks the shape; mixing shapes fails validation (65):

| Transport | Required | Allowed | Forbidden |
|---|---|---|---|
| `stdio` | `command` | `args`, `env`, `timeout`, `always_load`, `cwd` | `url`, `headers`, `headers_helper`, `oauth` |
| `http` / `sse` | `url` (http(s) scheme) | `headers`, `timeout`, `always_load`, `headers_helper`, `[server.oauth]` | `command`, `args`, `env`, `cwd` |
| `ws` | `url` (ws(s) scheme) | `headers`, `timeout`, `always_load`, `headers_helper` | `command`, `args`, `env`, `cwd`, `oauth` |

`env` keys must match `[A-Za-z_][A-Za-z0-9_]*`.

`timeout`, `always_load`, `headers_helper`, and `cwd` are additive
refinement fields — omitting them serializes identically to a descriptor
that never adopted them (an older grim reads a newer descriptor without
them unchanged). Each projects to a **subset** of clients; every other
client drops it silently, nothing auth-critical lost:

| Field | Projects for | Notes |
|---|---|---|
| `timeout` | Claude (`timeout`), OpenCode (`timeout`) | Startup/tool-fetch timeout, milliseconds |
| `always_load` | Claude (`alwaysLoad`) only | Load the server eagerly at client startup |
| `headers_helper` | Claude (`headersHelper`) only | Executable that produces fresh auth headers |
| `cwd` | OpenCode (`cwd`) only | Working directory for the launched process (stdio only) |

### The `server.oauth` block

`http`/`sse` only, every field optional — a structured OAuth client
config, projected for **Claude only**:

| Field | Type | Notes |
|---|---|---|
| `client_id` | string | May reference `${VAR}` |
| `scopes` | string list | Values may reference `${VAR}` |
| `callback_port` | integer | Fixed localhost callback port for the auth redirect |
| `auth_server_metadata_url` | string | RFC 8414 metadata URL; https-only; may reference `${VAR}` |

Deliberately **no `client_secret` field** — same rationale as `${VAR}`
env references: a secret has no safe home in a published artifact.

## Env References

Values may reference host environment variables with the canonical
`${VAR}` form — never a literal secret. Grim translates the reference
per client at install time (`{env:VAR}` for OpenCode, `${env:VAR}` for
the VS Code config; Claude reads `${VAR}` natively).

- `${VAR:-default}` is **rejected** — only Claude supports
  defaults natively, so a default would behave differently per client.
- Copilot CLI's global `mcp-config.json` supports no substitution at
  all: a descriptor that uses `${VAR}` skips that client with a warning
  rather than ever writing a secret (or a broken literal) to disk.
- Codex's `config.toml` receives a stdio `env` value **verbatim** — the
  literal `${VAR}` string is written as the launched subprocess's OS
  environment assignment (the same passthrough Claude/OpenCode give it),
  not substituted by grim or Codex. Remote `headers` map onto Codex's
  three surfaces: a literal value → `http_headers`, a whole-value
  `${VAR}` → `env_http_headers`, `Authorization: Bearer ${VAR}` →
  `bearer_token_env_var`; a header embedding a ref in surrounding text
  (or several refs) has no faithful target and skips Codex with a
  warning.
- A bare `$VAR` (no braces) is a literal, not a reference.

## What Each Client Receives

Grim renders the vendor's own schema — confirm the authoritative matrix
on the docs site ([MCP Server Artifacts][mcp-docs]). Highlights: OpenCode
gets `command` as ONE array (`["grim", "mcp"]`) under `type: "local"`
with env under `environment`; the VS Code config uses `type: "stdio"`;
Copilot CLI's global entry gains `tools: ["*"]`; Codex gets a
`[mcp_servers.<name>]` TOML table with `command`/`args`/`env` (stdio) or
`url` + the three header surfaces (http/sse). The `ws` transport and the
`[server.oauth]` block project for **Claude only** — every other client
skips such a descriptor with a warning. Only the managed entry
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
- `oauth` on `stdio`/`ws`, `cwd` on a remote, `headers_helper` on stdio,
  or a non-https `auth_server_metadata_url` → 65.
- `${VAR:-fallback}`, `${1BAD}`, `${UNCLOSED` anywhere in a string value → 65.
- Not a bundle member: bundles carry only `[skills]`/`[rules]`/`[agents]`
  tables — MCP descriptors cannot be listed in one; declare them directly.

An MCP descriptor's layer is a single JSON document — it carries no
in-tree README. For a readme/logo/changelog on the *repository*, publish
a description companion — see
[release-checklist.md](release-checklist.md#description-companion).

[mcp-docs]: https://grimoire.rs/mcp-servers.html
