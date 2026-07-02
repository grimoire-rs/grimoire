# MCP Server Artifacts

Skills teach a capability, rules constrain behavior, and agents define a
delegatable assistant. An **MCP server artifact** describes a fourth
thing: a running [Model Context Protocol][mcp-spec] server — how to
launch it, or how to reach it — so every client can connect to the same
tool without anyone hand-writing its config three times.

Adding one MCP server today means editing three different files by hand:
[Claude Code][claude-code-mcp-docs]'s `.mcp.json` (`mcpServers`, a
`command`/`args` pair), [OpenCode][opencode-mcp-docs]'s `opencode.json`
(`mcp`, a single `command` array), and [VS Code][vscode-mcp-docs]'s
`.vscode/mcp.json` (`servers`, yet another shape) — each with its own
environment-variable reference syntax (`${VAR}`, `{env:VAR}`,
`${env:VAR}`). The three copies drift the moment someone rotates a token
or renames a server, exactly the copy-paste problem
[skills][vendor-metadata] already solve for capabilities.

Grimoire treats an MCP server like any other artifact: author **one
canonical file**, publish it once, and let `grim install` register a
client-native entry in each detected client's own MCP config — spliced
in without disturbing a single byte outside that entry. Unlike a skill,
rule, or agent, an MCP descriptor never materializes as a file of its
own; see [What each client receives](#emit-matrix).

## The canonical format {#format}

An MCP server is a single `mcp/<name>.toml` file; the descriptor name is
the file stem, the same convention a [rule](./concepts.md) or
[agent](./agents.md) uses. `description` and the `[server]` table are
the only required parts:

```toml
# mcp/grim.toml
description = "Grimoire catalog search and install status over the Model Context Protocol."
summary = "grim as an MCP server (read-only catalog + status)"
keywords = "grimoire,mcp,catalog,search,status"
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
| `repository` | no | string | HTTPS source URL, same [validation](./publishing.md#metadata-repository) as every other kind (`org.opencontainers.image.source`) |
| `deprecated` | no | string | [Deprecation notice](./publishing.md#metadata-deprecated) (`com.grimoire.deprecated`) |
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
| `transport` | always | `stdio`, `http`, or `sse` |
| `command` | `stdio` | The executable to launch |
| `args` | `stdio`, optional | Arguments appended to `command` |
| `env` | `stdio`, optional | String→string map; values may reference the host environment as `${VAR}` |
| `url` | `http`/`sse` | Must start with `http://` or `https://` |
| `headers` | `http`/`sse`, optional | String→string map, same `${VAR}` referencing as `env` |

### Example — a remote server {#server-example-remote}

```toml
# mcp/acme-search.toml
description = "Acme's internal search index over MCP."

[server]
transport = "http"
url = "https://mcp.acme.internal/search"
headers = { Authorization = "Bearer ${ACME_MCP_TOKEN}" }
```

`sse` takes the same shape as `http` — [Server-Sent Events][mcp-spec]
transport is deprecated upstream in the MCP spec but still accepted by
every client Grimoire supports, so grim keeps it as a first-class
transport value.

## Validation {#validation}

`grim build`, `grim release`, and `grim publish` validate a descriptor
before it ever reaches a registry. Every violation below exits with code
65 (data error):

| Violation | Result |
|---|---|
| `description` empty or missing | rejected |
| `stdio` with no `command`, or `stdio` with `url`/`headers` set | rejected |
| `http`/`sse` with no `url`, or with `command`/`args`/`env` set | rejected |
| `url` not starting with `http://` or `https://` | rejected |
| an `env` key outside `[A-Za-z_][A-Za-z0-9_]*` | rejected |
| a malformed `${…}` reference — unclosed, an invalid variable name, or a `${VAR:-default}` fallback | rejected |
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
client already reads — never a new file:

| Client | Scope | File | Container key | Entry shape | Env-ref syntax |
|---|---|---|---|---|---|
| [Claude Code][claude-code-mcp-docs] | project | `<workspace>/.mcp.json` | `mcpServers` | `stdio`: `command`/`args`/`env` (no `type`); remote: `type: http\|sse` + `url` + `headers` | `${VAR}` (native, no translation) |
| [Claude Code][claude-code-mcp-docs] | global | `~/.claude.json` (`$CLAUDE_CONFIG_DIR/.claude.json` when set) | `mcpServers` | same as project | `${VAR}` |
| [OpenCode][opencode-mcp-docs] | project | `<workspace>/opencode.json` (or `.jsonc` when present) | `mcp` | local: `type: "local"`, `command` as **one** array (`[cmd, ...args]`), `environment`, `enabled: true`; remote: `type: "remote"`, `url`, `headers`, `enabled` | `{env:VAR}` |
| [OpenCode][opencode-mcp-docs] | global | `$OPENCODE_CONFIG` else the XDG default `opencode.json` | `mcp` | same as project | `{env:VAR}` |
| [VS Code][vscode-mcp-docs] (Copilot Chat) | project | `<workspace>/.vscode/mcp.json` | `servers` | `type: "stdio"` + `command`/`args`/`env`; `type: "http"\|"sse"` + `url`/`headers` | `${env:VAR}` |
| [Copilot CLI][copilot-mcp-docs] | global | `$COPILOT_HOME`\|`~/.copilot`/`mcp-config.json` | `mcpServers` | `type: "local"` + `command`/`args`/`env` + `tools: ["*"]`; `type: "http"\|"sse"` + `url`/`headers` + `tools` | **none** — see [Environment references](#env-references) |

Every write here is a **splice**, not a rewrite: grim locates the
existing `<container>.<member>` span in the file — parsing the
surrounding JSON/JSONC only enough to find that one member — and
replaces just that span, so key order, unrelated entries, formatting,
and JSONC comments elsewhere in the file all survive byte-for-byte. The
idiom is the same one [Ansible's `blockinfile` module][ansible-blockinfile]
uses for editing a marked region of a config file it does not own — grim
never owns `~/.claude.json` or `opencode.json`, so it never reserializes
them.

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
| [Copilot CLI][copilot-mcp-docs] (global) | not supported |

[Copilot CLI][copilot-mcp-docs]'s global `mcp-config.json` has no
variable-substitution mechanism at all — there is no syntax to translate
into. A descriptor with any `${VAR}` reference **skips** that one client
with a warning rather than ever inlining the resolved secret value into
a file on disk. Every other client and scope still installs normally;
only the Copilot-global registration is omitted.

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
[The five kinds](./artifacts.md#kinds) for why the annotation exists.
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
descriptor — see [`grim mcp`](./commands.md#mcp) for what the server
itself exposes once registered.

## Limitations {#limitations}

- **No `${VAR:-default}` support.** Only [Claude Code][claude-code-mcp-docs]
  supports inline defaults natively; v1 rejects the syntax entirely
  rather than honor it inconsistently across clients.
- **MCP descriptors cannot be bundle members yet.** A [bundle](./concepts.md#bundles)
  accepts skill, rule, and agent members only.
- **No per-vendor override keys.** Unlike a skill's or agent's
  `<vendor>.<field>` [metadata extensions](./vendor-metadata.md), an MCP
  descriptor has no escape hatch for a capability only one client
  understands — the format is deliberately vendor-agnostic.
- **VS Code's user-profile `mcp.json` (global VS Code, outside Copilot
  CLI) is not written.** Global Copilot registration always targets
  Copilot CLI's own `mcp-config.json`.
- **Copilot CLI's global config skips descriptors with `${VAR}`
  references** — see [Environment references](#env-references). Every
  other client and scope still installs normally.
- **A custom `$OPENCODE_CONFIG` outside every known root cannot be
  recorded portably.** grim stores each install record relative to a
  known root (the workspace, or a client's own config directory) so the
  record still resolves on another machine or container. An
  `$OPENCODE_CONFIG` pointed somewhere else entirely has no such root to
  record against, so that client is skipped for this install with a
  warning rather than recorded in a way that would silently break on a
  different host.

<!-- external -->
[mcp-spec]: https://spec.modelcontextprotocol.io/
[claude-code-mcp-docs]: https://code.claude.com/docs/en/mcp
[opencode-mcp-docs]: https://opencode.ai/docs/mcp-servers/
[vscode-mcp-docs]: https://code.visualstudio.com/docs/copilot/chat/mcp-servers
[copilot-mcp-docs]: https://docs.github.com/en/copilot/concepts/about-model-context-protocol-mcp
[ansible-blockinfile]: https://docs.ansible.com/ansible/latest/collections/ansible/builtin/blockinfile_module.html
[catalog-mcp-grim]: https://github.com/grimoire-rs/grimoire/blob/main/catalog/mcp/grim.toml

<!-- internal -->
[vendor-metadata]: ./vendor-metadata.md
