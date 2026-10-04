---
title: "MCP Server Artifacts"
description: "Skills teach a capability, rules constrain behavior, and agents define a persona — MCP servers extend an agent with tools."
---
<!-- doc_type: reference -->

Skills teach a capability, rules constrain behavior, and agents define a
delegatable assistant. An **MCP server artifact** describes a fourth
thing: a running [Model Context Protocol][mcp-spec] server — how to
launch it, or how to reach it — so every client can connect to the same
tool without anyone hand-writing its config three times.

## Why one canonical descriptor {#why-canonical}

Adding one MCP server today means editing a different config file by hand
for every client: [Claude Code][claude-code-mcp-docs]'s `.mcp.json`
(`mcpServers`, a `command`/`args` pair), [OpenCode][opencode-mcp-docs]'s
`opencode.json` (`mcp`, a single `command` array),
[VS Code][vscode-mcp-docs]'s `.vscode/mcp.json` (`servers`, yet another
shape), [GitHub Copilot][copilot-mcp-docs] CLI's `mcp-config.json` and
`.github/mcp.json`,
and [Codex][codex-mcp-docs]'s `config.toml` (`[mcp_servers.<name>]`, the
only TOML one) — each with its own shape and, where supported, its own
environment-variable reference syntax (`${VAR}`, `{env:VAR}`,
`${env:VAR}`). The copies drift the moment someone rotates a token or
renames a server, exactly the copy-paste problem
[skills][vendor-metadata] already solve for capabilities.

Grimoire treats an MCP server like any other artifact: author **one
canonical file**, publish it once, and let `grim install` register a
client-native entry in each detected client's own MCP config — spliced
in without disturbing a single byte outside that entry. Unlike a skill,
rule, or agent, an MCP descriptor never materializes as a file of its
own; see [What each client receives](#emit-matrix).

To register one descriptor into every client and read back each client's own
entry, follow [Register an MCP server everywhere](./guides/mcp-everywhere.md).
This page is the format reference behind it.

## The canonical format {#format}

An MCP server is a single `mcp/<name>.toml` file; the descriptor name is
the file stem, the same convention a [rule](./concepts.md) or
[agent](./agents.md) uses. `description` and the `[server]` table are
the only required parts:

```toml
# mcp/grim.toml
description = "Grimoire catalog, install status, and artifact content over the Model Context Protocol."
summary = "grim as an MCP server (search, status, fetch; render behind --allow-writes)"
keywords = "grimoire,mcp,catalog,search,status,fetch,render"
repository = "https://github.com/grimoire-rs/grimoire"

[server]
transport = "stdio"
command = "grim"
args = ["mcp"]
```

This is grim's own descriptor, published as `mcp/grim` — see
[Consuming](#consuming) for installing it.

### Common fields {#common-fields}

| Field | Required | Type | Notes |
|---|---|---|---|
| `description` | yes | string | Must be non-empty after trimming; becomes the OCI description annotation |
| `summary` | no | string | Short catalog blurb (`com.grimoire.summary`) |
| `keywords` | no | string | Comma-separated tags (`com.grimoire.keywords`) |
| `license` | no | string | SPDX-style identifier (`org.opencontainers.image.licenses`) |
| `repository` | no | string | HTTPS source URL, same [validation](./publishing.md#metadata-repository) as every other kind (`org.opencontainers.image.source`) |
| `deprecated` | no | string | [Deprecation notice](./publishing.md#metadata-deprecated) (`com.grimoire.deprecated`) |
| `replaced-by` | no | string | Reference [naming the successor](./publishing.md#metadata-replaced-by) (`com.grimoire.replaced-by`) |
| `authors` | no | string | [Who maintains it](./publishing.md#metadata-descriptive) (`org.opencontainers.image.authors`) |
| `vendor` | no | string | [Distributing organization](./publishing.md#metadata-descriptive) (`org.opencontainers.image.vendor`) |
| `homepage` | no | string | [Project home page](./publishing.md#metadata-descriptive) (`org.opencontainers.image.url`) |
| `documentation` | no | string | [Documentation URL](./publishing.md#metadata-descriptive) (`org.opencontainers.image.documentation`) |
| `server` | yes | table | The launch/connection definition, see [below](#server-table) |

Any field outside this table — at the top level or inside `[server]` —
is a hard parse error; there is no forward-compatible `extra` bucket
here the way a rule or skill has one.

### The `[server]` table {#server-table}

`transport` decides which of the remaining fields are legal. Mixing
shapes — a `url` on a `stdio` server, a `command` on an `http` one — is a
validation error, not a silent merge:

| Field | Required for | Notes |
|---|---|---|
| `transport` | always | `stdio`, `http`, `sse`, or `ws` |
| `command` | `stdio` | The executable to launch |
| `args` | `stdio`, optional | Arguments appended to `command` |
| `env` | `stdio`, optional | String→string map; values may reference the host environment as `${VAR}` |
| `url` | `http`/`sse`/`ws` | `http://` or `https://` for `http`/`sse`; `ws://` or `wss://` for `ws` |
| `headers` | `http`/`sse`/`ws`, optional | String→string map, same `${VAR}` referencing as `env` |
| `oauth` | `http`/`sse`, optional | OAuth client block, see [below](#server-oauth) — not valid for `ws` ([Claude documents no OAuth over WebSocket][claude-code-mcp-docs]) or `stdio` |
| `timeout` | any transport, optional | Startup/tool-fetch timeout in milliseconds — projected for [Claude Code][claude-code-mcp-docs] (`timeout`), [OpenCode][opencode-mcp-docs] (`timeout`), [Qoder][qoder-mcp-docs] (`timeout`), and [Codex][codex-mcp-docs] (`startup_timeout_ms`); dropped for clients without a native key. Also dropped for [Gemini CLI][gemini-docs], with a warning: Gemini applies its `timeout` to every tool call as well as startup, so a startup value would cut long tool calls short. Gemini's 10-minute default applies instead |
| `always_load` | any transport, optional | Load the server eagerly at client startup — projected for [Claude Code][claude-code-mcp-docs] (`alwaysLoad`) only |
| `headers_helper` | `http`/`sse`/`ws`, optional | Executable that produces fresh auth headers — projected for [Claude Code][claude-code-mcp-docs] (`headersHelper`) only |
| `cwd` | `stdio`, optional | Working directory for the launched process — projected for [OpenCode][opencode-mcp-docs] (`cwd`), [Gemini CLI][gemini-docs] (`cwd`), [Qoder][qoder-mcp-docs] (`cwd`), and [Warp][warp-mcp-docs] (`working_directory`) |

### Example — a remote server {#server-example-remote}

```toml
# mcp/acme-search.toml
description = "Acme's internal search index over MCP."

[server]
transport = "http"
url = "https://mcp.acme.internal/search"
headers = { Authorization = "Bearer ${ACME_MCP_TOKEN}" }
```

`sse` takes the same shape as `http`. The [HTTP+SSE transport][mcp-spec]
is deprecated in the MCP spec but not removed, so grim keeps it as a
first-class transport value. How each client receives it is in
[What each client receives](#emit-matrix). [Codex][codex-mcp-docs] has no
SSE transport, so grim writes an `sse` descriptor there as a
streamable-HTTP `url` entry.

`ws` describes a persistent WebSocket server (`wss://` canonical). Only
[Claude Code][claude-code-mcp-docs] reads a `type: "ws"` entry — every
other client [declines the descriptor](#emit-matrix) with a warning.

### The `[server.oauth]` block {#server-oauth}

A remote `http`/`sse` server that authenticates via OAuth can carry a
structured client configuration. All fields are optional:

```toml
[server]
transport = "http"
url = "https://mcp.acme.internal/search"

[server.oauth]
client_id = "${ACME_OAUTH_CLIENT_ID}"
scopes = ["search:read"]
callback_port = 43110
auth_server_metadata_url = "https://auth.acme.internal/.well-known/oauth-authorization-server"
```

| Field | Type | Notes |
|---|---|---|
| `client_id` | string | Pre-registered OAuth client id; may reference the host environment as `${VAR}` |
| `scopes` | string list | Requested scopes; `${VAR}` references allowed |
| `callback_port` | integer | Fixed localhost callback port for the authorization redirect |
| `auth_server_metadata_url` | string | Authorization-server metadata URL (RFC 8414); **https-only** |

There is deliberately **no `client_secret` field**: a secret has no safe
home in a published artifact — the same principle behind [`${VAR}`
environment references](#env-references).

A client writes the block only when it has a native target for every field
the block sets. Otherwise grim skips the server for that client, and the
warning names the fields it could not carry. Dropping a scope or a pinned
metadata URL would let the client ask for more than the author did, so grim
never writes a partial block:

| Client | Fields it carries | Written as |
|---|---|---|
| [Claude Code][claude-code-mcp-docs] | all four | `oauth.clientId`, `oauth.scopes`, `oauth.callbackPort`, `oauth.authServerMetadataUrl` |
| [OpenCode][opencode-mcp-docs] | `client_id`, `scopes`, `callback_port` | `oauth.clientId`, `oauth.scope` (the scopes joined by spaces), `oauth.callbackPort` |
| [Zed][zed-docs] | `client_id` | `oauth.client_id` |
| [Droid][droid-mcp-docs] | `client_id` (a `${VAR}` id is written as is; Droid expands it) | `oauth.clientId` |
| [Copilot][copilot-mcp-docs] | `client_id`, a literal value only (a `${VAR}` id skips) | `oauthClientId` in `.github/mcp.json` and `mcp-config.json`; `oauth.clientId` in `.vscode/mcp.json` |

Every other client [skips a descriptor that carries the block](#limitations).
A block that sets no fields writes nothing and never causes a skip.

## Validation {#validation}

`grim build`, `grim release`, and `grim publish` validate a descriptor
before it ever reaches a registry. Every violation below exits with code
65 (data error):

| Violation | Result |
|---|---|
| `description` empty or missing | rejected |
| `stdio` with no `command`, or `stdio` with `url`/`headers` set | rejected |
| `http`/`sse`/`ws` with no `url`, or with `command`/`args`/`env` set | rejected |
| a `url` scheme not fitting the transport — `http(s)://` for `http`/`sse`, `ws(s)://` for `ws` | rejected |
| an `env` key outside `[A-Za-z_][A-Za-z0-9_]*` | rejected |
| a malformed `${…}` reference — unclosed, an invalid variable name, or a `${VAR:-default}` fallback | rejected |
| `headers_helper` on `stdio`, or `cwd` on a remote transport | rejected |
| `oauth` on `stdio` or `ws`, or a non-`https://` `auth_server_metadata_url` | rejected |
| a non-`https://` `repository` | rejected (same gate as [every other kind](./publishing.md#metadata-repository)) |
| any field not in the [common](#common-fields) or [server](#server-table) tables | rejected |

`${VAR:-default}` is rejected deliberately: only
[Claude Code][claude-code-mcp-docs] supports inline defaults natively, so
honoring the syntax for one client and dropping it for the others would
make the same descriptor behave differently depending on which client
installed it. A bare `$VAR` with no braces is not treated as a reference
at all — it passes through untouched, so a value the launched process
itself expands (a shell-style default, say) is safe to author.

## What each client receives {#emit-matrix}

On `grim install`, each detected client renders the descriptor into its
own schema and splices the result into whichever MCP config file that
client already reads — never a new file. The concrete file paths below are
**not** part of the stability contract — vendor render layout may change in any
minor release (see [stability][stability-unstable]).

| Client | Scope | File | Container key | Entry shape | Env-ref syntax |
|---|---|---|---|---|---|
| [Claude Code][claude-code-mcp-docs] | project | `<workspace>/.mcp.json` | `mcpServers` | `stdio`: `command`/`args`/`env` (no `type`); remote: `type: http\|sse\|ws` + `url` + `headers`; refinements: `timeout`/`alwaysLoad`/`headersHelper`; oauth: `{clientId, callbackPort, authServerMetadataUrl, scopes}` | `${VAR}` (native, no translation) |
| [Claude Code][claude-code-mcp-docs] | global | `~/.claude.json` (`$CLAUDE_CONFIG_DIR/.claude.json` when set) | `mcpServers` | same as project | `${VAR}` |
| [OpenCode][opencode-mcp-docs] | project | `<workspace>/opencode.json` (or `.jsonc` when present) | `mcp` | local: `type: "local"`, `command` as **one** array (`[cmd, ...args]`), `environment`, `enabled: true`; remote: `type: "remote"`, `url`, `headers`, `enabled`, [`oauth`](#server-oauth); refinements: `timeout`/`cwd` | `{env:VAR}` |
| [OpenCode][opencode-mcp-docs] | global | `$OPENCODE_CONFIG` else the XDG default `opencode.json` (or `.jsonc` when present) | `mcp` | same as project | `{env:VAR}` |
| [VS Code][vscode-mcp-docs] (Copilot Chat) | project | `<workspace>/.vscode/mcp.json` | `servers` | `type: "stdio"` + `command`/`args`/`env`; `type: "http"\|"sse"` + `url`/`headers`; oauth: `oauth: {clientId}` | `${env:VAR}` |
| [Copilot CLI][copilot-mcp-docs] | project | `<workspace>/.github/mcp.json` — written beside the VS Code file, which the CLI does not read | `mcpServers` | same as global | `${VAR}` (identity — Copilot CLI expands it) |
| [Copilot CLI][copilot-mcp-docs] | global | `$COPILOT_HOME`\|`~/.copilot`/`mcp-config.json` | `mcpServers` | `type: "local"` + `command`/`args`/`env` + `tools: ["*"]`; `type: "http"\|"sse"` + `url`/`headers` + `tools`; oauth: `oauthClientId` | `${VAR}` (identity — Copilot CLI expands it) |
| [Codex][codex-mcp-docs] | project | `<workspace>/.codex/config.toml` | `mcp_servers` | `stdio`: `command`/`args`/`env`; remote: `url` + headers mapped onto `http_headers` (static) / `env_http_headers` (whole-value `${VAR}`) / `bearer_token_env_var` (`Authorization: Bearer ${VAR}`) — see [Limitations](#limitations) for the residual skip; refinement: `timeout` → `startup_timeout_ms` | `${VAR}` (literal passthrough, not substituted by grim) |
| [Codex][codex-mcp-docs] | global | `$CODEX_HOME`\|`~/.codex`/`config.toml` | `mcp_servers` | same as project | same as project |
| [Cursor][cursor-docs] | project / global | `.cursor/mcp.json` / `~/.cursor/mcp.json` | `mcpServers` | `stdio`: `type: "stdio"` + `command`/`args`/`env`; remote: `url` + `headers`; oauth skipped | `${env:VAR}` (grim translates `${VAR}`) |
| [Kiro][kiro-docs] | project / global | `.kiro/settings/mcp.json` / `$KIRO_HOME`\|`~/.kiro`/`settings/mcp.json` | `mcpServers` | `stdio`: `command`/`args`/`env` (no `type`); oauth skipped | `${VAR}` (native passthrough) |
| [Junie][junie-docs] | project / global | `.junie/mcp/mcp.json` / `~/.junie/mcp/mcp.json` (`$JUNIE_HOME/mcp/mcp.json` when set) | `mcpServers` | `stdio`: `command`/`args`/`env`; oauth skipped | undocumented — ref-bearing descriptors skipped |
| [Gemini CLI][gemini-docs] | project / global | `<workspace>/.gemini/settings.json` / `$GEMINI_CLI_HOME`\|`$HOME` + `/.gemini/settings.json` — the `.gemini` segment is appended either way | `mcpServers` | `stdio`: `command`/`args`/`env`/`cwd`; `sse`: `url`; `http`: `httpUrl`; `timeout` dropped with a warning; oauth skipped | `${VAR}` (native passthrough) |
| [Zed][zed-docs] | project / global | `.zed/settings.json` / the platform-resolved Zed config root + `/settings.json` (JSONC) — `$XDG_CONFIG_HOME`\|`~/.config`/`zed` on Linux and FreeBSD, a hardcoded `~/.config/zed` on macOS, `%APPDATA%\Zed` on Windows | `context_servers` | flat `command`/`args`/`env` (no `type`); remote: `url` + `headers`, [`oauth`](#server-oauth) `client_id` only | none upstream — ref-bearing descriptors skipped |
| [Amp][amp-docs] | project / global | `.amp/settings.json` / `~/.config/amp/settings.json` | `amp.mcpServers` (literal dotted key) | `stdio`: `command`/`args`/`env`; oauth skipped | `${VAR_NAME}` (native passthrough) |
| [Antigravity][antigravity-docs] | project / global | `.agents/mcp_config.json` / `~/.gemini/config/mcp_config.json` | `mcpServers` | `stdio`: `command`/`args`/`env`; remote (`sse`, `http`): `serverUrl` + `headers`; ws + oauth skipped | undocumented — ref-bearing descriptors skipped |
| [Warp][warp-mcp-docs] | project / global | `.warp/.mcp.json` / `~/.warp/.mcp.json` | `mcpServers` | `stdio`: `command`/`args`/`env`/`working_directory` (from `cwd`); remote (`sse`, `http`): `url` + `headers`; ws and any oauth field skipped | undocumented — ref-bearing descriptors skipped |
| [Qoder][qoder-mcp-docs] | project / global | `.qoder/settings.json` / `$QODER_CONFIG_DIR`\|`~/.qoder`/`settings.json` — never the shared `.mcp.json` Qoder also reads, which grim manages for Claude Code | `mcpServers` | `stdio`: `command`/`args`/`env`/`cwd` (no `type`); remote (`sse`, `http`): `type` + `url` + `headers`; `timeout` (ms); ws + oauth skipped | undocumented — ref-bearing descriptors skipped |
| [Cline][cline-mcp-docs] | global only — no project file | `$CLINE_MCP_SETTINGS_PATH`, else `$CLINE_DATA_DIR/settings/cline_mcp_settings.json`, else `$CLINE_DIR`\|`~/.cline` + `/data/settings/cline_mcp_settings.json` — shared by the Cline CLI and VS Code extension, edited under Cline's own lock | `mcpServers` | flat `type: "stdio"` + `command`/`args`/`cwd`/`env`; `type: "streamableHttp"\|"sse"` + `url`/`headers`; ws + oauth skipped | `${env:VAR}` (the extension expands it, the CLI does not) |
| [Droid][droid-mcp-docs] | project / global | `.factory/mcp.json` / `~/.factory/mcp.json` | `mcpServers` | `type: "stdio"` + `command`/`args`/`env`; `type: "http"\|"sse"` + `url` + `headers`; oauth: `{clientId}` when `client_id` is the only field set, else skipped; ws skipped | `${VAR}` (native) in `env`, `headers` and `oauth.clientId` only — a reference in `command`, `args` or `url` skips the server |

Some clients are absent from the table because grim writes no MCP config for
them at all — they decline the kind, so grim warns, skips, and writes nothing.

The vendor-neutral `agents` client has no MCP surface to write: there is no
cross-vendor standard MCP config location. `goose`,
`openclaw` and `kilo` write no MCP config either. Two of those declines are
grim-side rather than upstream gaps, and are worth naming: `goose` keeps its
MCP servers in a **YAML** `config.yaml`, and `openclaw` in a file mixing strict
JSON with JSON5 — grim's config splicer handles JSON and TOML only, and
editing either format without a matching engine risks corrupting settings the
user did not ask grim to touch.

`warp` also reads other agents' MCP files: Claude Code's `.mcp.json` and
`~/.claude.json`, and `.agents/.mcp.json`. With both `claude` and `warp`
selected, one server can register twice in Warp. Warp starts a project-scoped
server only after you approve it (see [Warp: MCP][clients-warp-mcp]).

### Install order alongside a hook {#install-order-hooks}

[Qoder][qoder-mcp-docs] is the one client where an MCP entry and a
[hook](./hooks.md) registration land in the **same** file, at **global
scope**. Hooks arm on Qoder at global scope only, and hooks are
experimental. See [Stability](./stability.md#unstable).

Both entries land in Qoder's own `settings.json`, beside the user's own
hand-written keys.

`grim install` splices both independently of order. Arming a hook touches
only the marked hook element it owns. An MCP install touches only
`mcpServers`. Neither pass rewrites bytes the other wrote or the user did.

Re-running either in either order writes zero additional bytes once both
are already current. Removing one, through `grim uninstall` or the hook
feature flag going back off, leaves the other exactly as it was.

Codex is the one **TOML** target — every other client above writes
JSON/JSONC — so its splice runs through a separate span-preserving
engine built on [`toml_edit`][toml-edit-crate] instead of grim's own
JSON/JSONC scanner; the entry still lands under a `[mcp_servers.<name>]`
table the same way a JSON entry lands under `mcpServers.<name>`. Codex
also only honors a *project*-scope `.codex/config.toml` for a project the
user has marked **trusted** — grim writes the file regardless of trust
state, since that decision is Codex's, not grim's, to make; an untrusted
project simply will not have the registration read until it is trusted.

Every write here is a **splice**, not a rewrite: grim locates the
existing `<container>.<member>` span in the file — parsing the
surrounding JSON/JSONC (or, for Codex, TOML) only enough to find that one
member — and replaces just that span, so key order, unrelated entries,
formatting, and comments elsewhere in the file all survive byte-for-byte.
The idiom is the same one [Ansible's `blockinfile` module][ansible-blockinfile]
uses for editing a marked region of a config file it does not own — grim
never owns `~/.claude.json`, `opencode.json`, or `config.toml`, so it
never reserializes them.

That matters most for `~/.claude.json`: it is Claude Code's live,
monolithic user-state file, not a config file grim can treat as its own.
A parse-and-reserialize write would risk clobbering unrelated state a
running Claude session just wrote; the span-preserving splice touches
only the one `mcpServers.<name>` member. A concurrent edit by a running
Claude session while grim writes is last-writer-wins on that one member
— the same exposure any tool editing a shared config file has.

## Environment references {#env-references}

A descriptor authors environment and header values with the canonical
`${VAR}` form — [Claude Code][claude-code-mcp-docs]'s native syntax.
Every other client's writer translates it at render time; only string
leaves are rewritten, so `${VAR}` embedded inside a longer string (a URL
query parameter, say) still translates correctly:

| Client | Rendered form |
|---|---|
| [Claude Code][claude-code-mcp-docs] | `${VAR}` (identity — no translation) |
| [OpenCode][opencode-mcp-docs] | `{env:VAR}` |
| [VS Code][vscode-mcp-docs] (Copilot Chat) | `${env:VAR}` |
| [Copilot CLI][copilot-mcp-docs] (global) | `${VAR}` (identity — Copilot CLI expands it itself) |
| [Droid][droid-mcp-docs] | `${VAR}` (identity — Droid expands it in `env`, `headers` and `oauth.clientId`) |
| [Codex][codex-mcp-docs] | `${VAR}` (literal passthrough — an `env` value is an OS environment assignment for the launched subprocess, not substituted by grim or Codex) |
| [Cline][cline-mcp-docs] | `${env:VAR}` (the VS Code extension expands it; the Cline CLI passes it on literally) |

[Copilot CLI][copilot-mcp-docs] expands `${VAR}` in its global
`mcp-config.json` itself, in `command`, `args`, `env`, `url` and `headers`
(verified against CLI 1.0.88). grim writes the reference as authored and
never its value. Five upstream behaviors are worth knowing:

- An unset variable is passed on literally as `${VAR}`, not as an empty string.
- Copilot parses `url` before it expands it. A reference in the port
  therefore makes it drop the entry. grim skips such a descriptor for Copilot's global
  config with a warning. A reference in the host or the path works.
- Copilot also expands a bare `$NAME`, so a literal `$` word in an argument
  or value can be substituted there.
- Copilot CLI releases before about April 2026, 0.0.406 aside, pass an
  `env` reference on unexpanded, so the server sees the literal `${VAR}`.
- Unlike Claude Code, Copilot does not blank well-known credential
  variables, so a `${GITHUB_TOKEN}` in a `url` or header reaches that host.

One invalid entry makes [Cline][cline-mcp-docs] reject its **whole**
settings file. Its CLI also checks `url` without expanding it. So
grim skips a descriptor for Cline, with a warning, when its `url` holds a
reference and is not a URL as written. A reference in the host or port does
that, and one in the path does not.

Before 2026-09-27 grim wrote no Cline MCP
entry at all. The next `grim install --global` adds it.

Until 2026-09-27 grim skipped the Copilot-global registration for any
descriptor with a `${VAR}` reference. The next `grim install` adds it, and
`grim status` lists it under `outputs_pending` until then.

[Claude Code][claude-code-mcp-docs] reads a fixed set of well-known
credential variables as empty in a remote server's `url` and `headers`.
Examples are `ANTHROPIC_API_KEY`, `AWS_BEARER_TOKEN_BEDROCK` and
`NPM_TOKEN`. The server then usually fails with a 401. Name the variable
after the server instead, such as `${ACME_MCP_TOKEN}`. grim does not
rename it for you: it writes the reference exactly as authored.

## Semantic modification detection {#modification-detection}

A materialized skill or rule is judged modified by hashing the bytes on
disk. An MCP entry lives inside a file grim does not own, where the
surrounding content — key order, indentation, unrelated servers — can
legitimately change without grim's entry changing at all. So the
integrity record hashes the **rendered entry value** instead: canonical,
sorted-key JSON, independent of how the value happens to be formatted or
where it sits in the file.

Three consequences follow directly:

- **Reordering keys or reformatting the file** — even the managed
  entry's own keys — never flags `modified`.
- **Changing the managed value** (a different `command`, a rotated
  `Authorization` header) is a real modification; `grim install` refuses
  to overwrite it without `--force`, the same integrity gate every other
  kind uses.
- **Deleting the entry** while the file itself stays intact reads as
  `missing`, not `modified` — the same distinction [`grim status`](./commands.md#status)
  already makes for a deleted skill or rule file.

[`grim status`](./commands.md#status), the [TUI](./commands.md#tui), and
the install integrity gate all share this one check — there is no
separate code path for MCP entries.

## Publishing {#publishing}

`grim build` and `grim release` need `--kind mcp` for an MCP descriptor
(`grim publish` needs no flag — an entry's kind is structural, fixed by
which manifest table it sits in, see [below](#publishing-manifest)):

```sh
grim build ./mcp/acme-search.toml --kind mcp
grim release ./mcp/acme-search.toml ghcr.io/acme/mcp/acme-search:1.0.0 --kind mcp
```

The flag is required because a `.toml` path is [bundle](./concepts.md#bundles)-shaped
by default — bundles are the other `.toml` artifact kind. When a `.toml`
file carries a `[server]` table but no `--kind mcp` flag, grim reports
that it looks like an MCP descriptor and asks for the flag, mirroring the
hint a [publish manifest pointed at `grim release`](./publishing.md#batch-publish-disambiguation)
gets.

On the wire, an MCP descriptor publishes as a single canonical-JSON layer
(media type `application/vnd.grimoire.mcp.v1+json`, capped at 64 KiB),
the same OCI empty config every kind uses, and the same
`com.grimoire.kind: mcp` manifest annotation — see
[The six kinds](./artifacts.md#kinds) for why the annotation exists.
Conventionally it publishes to `<registry>/<namespace>/mcp/<name>:<version>`,
the same `{kind-subdir}/{name}` layout every other kind uses by default.

### In a `publish.toml` manifest {#publishing-manifest}

A [`publish.toml`](./publishing.md#batch-publish) manifest gains an
`[mcp.<name>]` table alongside `[skills.<name>]`, `[rules.<name>]`,
`[agents.<name>]`, and `[bundles.<name>]`:

```toml
[mcp.acme-search]
version = "1.0.0"
repository = "https://github.com/acme/mcp-search"  # optional
```

The conventional source path — when `path` is omitted — is
`mcp/{name}.toml`, relative to the manifest's directory, alongside the
`skills/`, `rules/`, `agents/`, and `bundles/` conventions.
[`grim publish`](./commands.md#publish) releases entries in a fixed kind
order: **skills → rules → agents → mcp → bundles**, alphabetical within
each kind — mcp servers publish before bundles for the same reason
skills and rules do: a bundle may reference an already-published member.

## Consuming {#consuming}

MCP servers ride the standard lifecycle, with one difference at the
install step: there is no file to write, only a config entry to
register.

```sh
grim add ghcr.io/grimoire-rs/mcp/grim:1     # kind inferred from com.grimoire.kind
grim install                                 # registers an entry in every selected client
grim status                                  # shows the mcp row
grim uninstall mcp grim                      # removes only the managed entry, never the file
```

[`grim add`](./commands.md#add) declares the entry in the `[mcp]` table
of `grimoire.toml`; the lock carries a `[[mcp]]` array, emitted only when
non-empty so an mcp-free project's lock is byte-identical to before this
feature existed. [`grim uninstall`](./commands.md#uninstall) splices the
one managed member back out of each client's config file — the file
itself, and every other entry in it, is left exactly as it was. If the
managed member was the container's only entry (the only server under
`mcpServers`, say), the now-empty container key is dropped too rather
than leaving a `"mcpServers": {}` husk behind.

grim's own server is published at `ghcr.io/grimoire-rs/mcp/grim` from
the source descriptor [`catalog/mcp/grim.toml`][catalog-mcp-grim] shown
[above](#format), installable the same way as any third-party
descriptor. Once registered it exposes five tools — `grim_search`,
`grim_status`, `grim_fetch`, `grim_describe`, and (behind `--allow-writes`)
`grim_render` — each taking the install scope as optional per-call
arguments; `grim_status` also takes an optional `check` argument for a
live catalog re-check (network read, same as CLI `grim status --check`);
the full tool table lives at [`grim mcp`](./commands.md#mcp).

## Limitations {#limitations}

- **No `${VAR:-default}` support.** Only [Claude Code][claude-code-mcp-docs]
  supports inline defaults natively; v1 rejects the syntax entirely
  rather than honor it inconsistently across clients.
- **No per-vendor override keys.** Unlike a skill's or agent's
  `<vendor>.<field>` [metadata extensions](./vendor-metadata.md), an MCP
  descriptor has no escape hatch for a capability only one client
  understands — the format is deliberately vendor-agnostic.
- **VS Code's user-profile `mcp.json` (global VS Code, outside Copilot
  CLI) is not written.** Global Copilot registration always targets
  Copilot CLI's own `mcp-config.json`.
- **A workspace `.mcp.json` hides `.github/mcp.json` from Copilot CLI.** At
  project scope grim writes a Copilot server twice: into `.vscode/mcp.json`
  for VS Code's Copilot Chat, and into `.github/mcp.json` for the Copilot
  CLI, which does not read the VS Code file. When both `.mcp.json` and
  `.github/mcp.json` sit in the same directory, the CLI reads `.mcp.json`
  and ignores `.github/mcp.json` as a whole. That covers every server in it,
  not only the ones that share a name. `.mcp.json` is the file grim writes for
  [Claude Code][claude-code-mcp-docs], so with Claude Code also selected the
  CLI runs Claude Code's entry for the server instead of its own. A
  Claude-shaped stdio entry loads there (verified against CLI 1.0.88). A
  remote entry carrying Claude-only fields such as `oauth` or
  `headersHelper` is unverified. A server that only `.github/mcp.json`
  carries does not load.
- **Cline's own keys stay Cline's.** Cline writes `autoApprove`,
  `disabled`, `timeout`, `oauth`, `oauthClient`, `metadata` and
  `remoteConfigured` into each server entry. `grim status` does not count
  them as an edit, and an update keeps them. Uninstall removes the whole
  entry, those keys included.
- **Cline has no project-scope MCP file.** A project install skips Cline
  with a warning, so install the server with `--global`. grim also skips a
  settings file that `CLINE_MCP_SETTINGS_PATH`, `CLINE_DATA_DIR` or
  `CLINE_DIR` moves outside `~/.cline`. It could not record where it wrote.
  grim takes Cline's own lock on the file first. While another process
  holds it, the command exits 75 and is safe to retry.
- **`ws` transport is Claude-only.** [OpenCode][opencode-mcp-docs],
  [Copilot][copilot-mcp-docs] (both scopes), and [Codex][codex-mcp-docs]
  document no WebSocket MCP transport; a `transport = "ws"` descriptor is
  skipped for them with a warning rather than registering a remote entry
  they would try to speak HTTP to. Every other client declines it for the
  same reason. [Qoder][qoder-mcp-docs] documents a `ws` type, but it takes a
  `tcp` host-and-port object rather than a URL, so grim declines it there too.
  [Antigravity][antigravity-docs]'s MCP docs do name websocket
  alongside sse and streamable HTTP under one `serverUrl` field, but that
  single sentence could not be confirmed against raw upstream page text, and
  support is additive to add later while removal would be breaking — so it
  is declined there as well, pending confirmation.
- **The `[server.oauth]` block reaches few clients.** Claude Code carries
  every field, OpenCode all but `auth_server_metadata_url`, and Zed,
  [Copilot][copilot-mcp-docs] and [Droid][droid-mcp-docs] only `client_id` ([the table above](#server-oauth)).
  Any other client, or a field the client cannot carry, skips the server
  with a warning naming the fields: OAuth is auth-critical, so grim never
  writes a block that differs from the authored one. To reach Zed,
  Copilot and Droid, publish a variant that sets only `client_id`. Descriptors
  without the block are unaffected.
- **New descriptor fields do not parse on an older grim.** The descriptor
  layer is `deny_unknown_fields`: an artifact published with `timeout`,
  `always_load`, `headers_helper`, `cwd`, `oauth`, or `transport = "ws"`
  is **rejected by a grim build that predates those fields** (a data
  error, exit 65, at install/fetch time — same stance as
  [stability.md's forward-compatibility limitation][stability-forward]).
  Publishers who need to serve old clients should simply not author the
  new fields; a descriptor without them stays byte-identical across the
  version boundary.
- **[Codex][codex-mcp-docs] headers must fit one of three upstream
  surfaces.** A remote descriptor's `headers` map onto Codex's
  `http_headers` (a plain literal value), `env_http_headers` (a value
  that is exactly one `${VAR}`), or `bearer_token_env_var`
  (`Authorization: Bearer ${VAR}`). A header that embeds an env
  reference in surrounding text — or carries several references — has no
  faithful Codex representation, so that descriptor is skipped for Codex
  with a warning rather than writing a broken literal or inlining a
  secret. Every other client and scope still installs normally.
- **`grim_fetch` / `grim_describe` / `grim_render` need the network even
  with warm blobs.** grim's cache stores blobs but not manifests, so under
  `GRIM_OFFLINE=1` (or `--offline`) these fail cleanly at the manifest
  lookup (or, for an uncached reference, the digest resolution) instead of
  serving from cache. A manifest cache for true offline fetch is a tracked
  follow-up.
- **A custom `$OPENCODE_CONFIG` outside every known root cannot be
  recorded portably.** grim stores each install record relative to a
  known root (the workspace, or a client's own config directory) so the
  record still resolves on another machine or container. An
  `$OPENCODE_CONFIG` pointed somewhere else entirely has no such root to
  record against, so that client is skipped for this install with a
  warning rather than recorded in a way that would silently break on a
  different host.

<!-- internal -->
[stability-forward]: ./stability.md#limitations-forward-compat
[stability-unstable]: ./stability.md#unstable
[clients-warp-mcp]: ./clients.md#gap-warp-mcp

<!-- external -->
[mcp-spec]: https://modelcontextprotocol.io/specification/latest
[claude-code-mcp-docs]: https://code.claude.com/docs/en/mcp
[opencode-mcp-docs]: https://opencode.ai/docs/mcp-servers/
[vscode-mcp-docs]: https://code.visualstudio.com/docs/copilot/chat/mcp-servers
[copilot-mcp-docs]: https://docs.github.com/en/copilot/concepts/about-model-context-protocol-mcp
[codex-mcp-docs]: https://developers.openai.com/codex/mcp
[cursor-docs]: https://cursor.com
[kiro-docs]: https://kiro.dev
[junie-docs]: https://www.jetbrains.com/junie/
[gemini-docs]: https://geminicli.com
[zed-docs]: https://zed.dev
[amp-docs]: https://ampcode.com
[antigravity-docs]: https://antigravity.google/docs/mcp
[qoder-mcp-docs]: https://docs.qoder.com/cli/mcp-reference
[warp-mcp-docs]: https://docs.warp.dev/agents/capabilities/mcp/
[cline-mcp-docs]: https://docs.cline.bot/mcp/configuring-mcp-servers
[droid-mcp-docs]: https://docs.factory.ai/cli/configuration/mcp
[ansible-blockinfile]: https://docs.ansible.com/ansible/latest/collections/ansible/builtin/blockinfile_module.html
[catalog-mcp-grim]: https://github.com/grimoire-rs/grimoire/blob/main/catalog/mcp/grim.toml
[toml-edit-crate]: https://docs.rs/toml_edit/latest/toml_edit/

<!-- internal -->
[vendor-metadata]: ./vendor-metadata.md
