# Research: Qoder vendor verification

Verified 2026-09-24 against docs.qoder.com, for adding `qoder` as a grim
client. Qoder is Alibaba's agentic IDE plus `qodercli`; its config surface is a
near-clone of Claude Code's under `.qoder/`. Consumers:
`src/install/vendor_qoder.rs`, `docs/src/content/docs/clients.md` `{#gap-qoder}`,
the "Qoder watchlist" in `.claude/rules/vendor-capability-watchlist.md`.

## Verified layout

| Kind | Project | Global (`$QODER_CONFIG_DIR` else `~/.qoder`) | Shape | Source |
|---|---|---|---|---|
| skill | `.qoder/skills/<n>/SKILL.md` | `<root>/skills/<n>/` | agentskills `name`/`description` only; user-level overrides project on a name clash | [Skills](https://docs.qoder.com/cli/Skills) |
| rule | `.qoder/rules/**/*.md` | `<root>/rules/**/*.md` | `paths` (glob or list, ≡ `trigger: glob` + `glob`), also `trigger` (`always_on`/`manual`/`model_decision`/`glob`), `glob`, `alwaysApply`, `description` | [Memory](https://docs.qoder.com/cli/memory) |
| agent | `.qoder/agents/*.md` | `<root>/agents/*.md` | `name`, `description`, `model` (alias or `inherit`/`auto`/`lite`/`efficient`/`performance`; omitted = `inherit`), `tools` (comma string, inline or block list, `*`), `disallowedTools` | [Subagents](https://docs.qoder.com/cli/subagent) |
| mcp | `.qoder/settings.json` or `.mcp.json` | `<root>/settings.json` | `mcpServers.<name>`; see below | [MCP reference](https://docs.qoder.com/cli/mcp-reference), [Settings](https://docs.qoder.com/cli/settings) |

`QODER_CONFIG_DIR`: "The location of the User Configuration Directory can be
customized via the `QODER_CONFIG_DIR` Environment Variable" — replaces
`~/.qoder` outright, no segment appended
([Config scope](https://docs.qoder.com/cli/config-scope)).

## MCP entry shape

- stdio: `command` (required), `args`, `env`, `cwd`; `type` optional (stdio is
  the default).
- remote: `type` `sse` or `http`/`streamable-http`, `url`, `headers`.
- `ws`: takes a `tcp` object (`host`/`port`), not a URL.
- `timeout`: number, **milliseconds**.
- `oauth`: `{enabled, clientId, clientSecret, authorizationUrl, tokenUrl,
  scopes, callbackPort}` — no metadata-URL discovery field.
- `qoder_url`: managed-gateway routing (no grim equivalent).
- Env-var expansion (`${VAR}`): **not documented**.

## Gaps found

- **Support-dir over-load.** Rules load recursively, so a rule's sibling
  support directory under `rules/<name>/` loads as unscoped rules. The memory
  page names `agentsMdExcludes` once ("Check if `agentsMdExcludes` excludes the
  target file", troubleshooting for AGENTS.md) with no scope, format or
  settings file; the settings reference does not list it. Not usable yet.
- **`.agents/skills` pool.** Not mentioned anywhere — Qoder is not
  pool-capable.
- **IDE vs CLI.** Every page above documents `qodercli`. That the IDE reads the
  same `.qoder/` tree is inferred, not stated.
- **`.mcp.json`.** Documented as a project MCP source alongside
  `.qoder/settings.json`. grim writes only `.qoder/settings.json`, because
  `.mcp.json` is Claude's grim-managed file.
