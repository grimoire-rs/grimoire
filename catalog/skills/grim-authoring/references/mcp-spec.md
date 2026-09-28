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
grim registers a vendor-native entry in the MCP config file each client
already reads, and removes exactly that entry on uninstall. Most clients
have such a file, but not all: most of the skills-only clients ship no MCP
config surface, except Cline (global scope only), Droid, and Warp, which
register it — and the vendor-neutral `agents` target
declines the kind outright, so **skill is the only kind no client declines**. Check the
[client matrix][clients] before assuming your audience is covered. Codex's
config is TOML, not
JSON — grim splices it span-preserving the same way, so surrounding user
keys and comments survive.

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
| `timeout` | Claude (`timeout`), OpenCode (`timeout`), Qoder (`timeout`), Codex (`startup_timeout_ms`) | Startup/tool-fetch timeout, milliseconds. Gemini drops it with a warning: its `timeout` bounds every tool call too |
| `always_load` | Claude (`alwaysLoad`) only | Load the server eagerly at client startup |
| `headers_helper` | Claude (`headersHelper`) only | Executable that produces fresh auth headers |
| `cwd` | OpenCode (`cwd`), Gemini (`cwd`), Qoder (`cwd`), Warp (`working_directory`) | Working directory for the launched process (stdio only) |

### The `server.oauth` block

`http`/`sse` only, every field optional — a structured OAuth client
config, written only where a client carries **every** field it sets
(lossless-or-skip): Claude all four, OpenCode all but
`auth_server_metadata_url` (and `callback_port = 0` skips, since OpenCode
takes 1-65535), Zed a literal `client_id` alone (Zed expands no `${VAR}`,
so a referenced id skips). Anywhere else the server is skipped with a
warning naming the uncarried fields:

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
per client at install time: `{env:VAR}` for OpenCode, `${env:VAR}` for
the VS Code config, Cursor and Cline (only Cline's VS Code extension
expands it; its CLI does not); Claude, Kiro, Gemini, Amp, and Copilot
CLI's `mcp-config.json` and project `.github/mcp.json` read `${VAR}`
natively. Copilot parses a
remote `url` before expanding it, so a `${VAR}` in the URL port skips
that client; Cline does the same for a reference in the host or port. Exact per-client syntax: [env references][env-refs].

- `${VAR:-default}` is **rejected** — only Claude supports
  defaults natively, so a default would behave differently per client.
- **Five surfaces have no substitution mechanism at all** — Junie,
  Antigravity, Qoder, and Warp (interpolation undocumented upstream), and
  Zed. A descriptor carrying any `${VAR}` skips those clients with a
  warning rather than ever writing a secret (or a broken literal) to disk; every other client still installs
  normally. Budget for it: an env-referencing server reaches a smaller
  fleet than a self-contained one.
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

Grim renders each MCP-hosting client's own schema — container key, entry
shape, and file differ per client, and the authoritative matrix lives on
the docs site ([emit matrix][emit-matrix]). Not every client is in that
set: most of the skills-only clients (Cline at global scope only, Droid,
and Warp aside) and the vendor-neutral `agents` target ship
no MCP config surface, so a descriptor writes nothing for them. What matters while
*authoring*, rather than at install time:

- **`stdio`, `http`, and `sse` register for every client.** The `ws`
  transport projects for **Claude only**; the `[server.oauth]` block for
  OpenCode, Zed, Copilot, and Droid, and only when the client carries every
  field set. Every other client skips such a descriptor with a warning. Set
  just a literal `client_id` to reach all four (Copilot and Droid accept
  only a literal `client_id`); a `${VAR}` id, or
  `scopes`, drops Zed and Copilot; `auth_server_metadata_url` or
  `callback_port = 0` drops OpenCode too.
- **Shape differences are grim's problem, not yours.** OpenCode receives
  `command` as ONE array (`["grim", "mcp"]`), Codex a
  `[mcp_servers.<name>]` TOML table, Zed a flat entry under
  `context_servers`, Amp one under the literal dotted key
  `amp.mcpServers` — all from the same descriptor, no per-client
  authoring.

Only the managed entry is ever touched — user keys, formatting, and
comments in the config file survive, and grim's drift check is semantic
(reordering the file is not a modification; editing the entry's values
is).

**Plugin portability:** `grim export plugin` renders a descriptor into a
`.mcp.json`/`mcp.json` entry the same way install does, so the same
authoring rules apply there. A relative `command` path does not travel
inside an exported plugin (no project root to resolve it against). Use an
absolute path, a `PATH` command, or a plugin placeholder in `args`.

Plugin placeholders are renamed per family, in an export only (`grim
install` writes the descriptor as is):

| Export for | `${CLAUDE_PLUGIN_ROOT}` / `${CLAUDE_PLUGIN_DATA}` | `${PLUGIN_ROOT}` / `${PLUGIN_DATA}` |
|---|---|---|
| `claude` | kept | renamed to Claude's names, in every field incl. `url` and headers |
| `codex`, `cursor`, `copilot`, `agents` | renamed to the spec's names | kept |
| `droid`, `junie`, `openclaw` | kept | kept |

**Portable advice:** put `${PLUGIN_ROOT}` / `${PLUGIN_DATA}` (or the
Claude spelling) in `args` or `env` values. That works for `claude` and
every Agent Plugins client. The Claude rendering has no `cwd`.

For an Agent Plugins export (`codex`, `cursor`, `copilot`, `agents`), a
`${…}` reference in `command`, `url`, an env key or a header omits the
server `not-representable` (the spec expands nothing there). Any other
`${VAR}` in `args`, env values or `cwd` is exported with a warning: those
clients may pass it through literally.

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

The required `description` field is the descriptor's prose and becomes the
OCI `description` annotation. What the layer cannot carry is an *in-tree*
README: it is a single JSON document, not a file tree. Ship a
readme/logo/changelog on the repository as a description companion — see
[release-checklist.md](release-checklist.md#description-companion).

[emit-matrix]: https://grimoire.rs/mcp-servers.html#emit-matrix
[clients]: https://grimoire.rs/clients.html#matrix
[env-refs]: https://grimoire.rs/mcp-servers.html#env-references
