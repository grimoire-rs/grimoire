---
paths:
  - src/install/mcp_config.rs
  - src/install/json_splice.rs
  - src/install/toml_splice.rs
  - src/install/cline_lock.rs
  - src/install/vendor_agents.rs
---

# Install Layout: MCP Servers and Agents

Where an MCP entry and an agent materialize per client. Index: [subsystem-file-structure.md](./subsystem-file-structure.md).

### MCP servers {#install-layout-mcp}

An **mcp** artifact never materializes a file. Install registers a
vendor-native entry in each client's own MCP config file via a
span-preserving splice (every byte outside the managed member — key
order, formatting, comments — survives): JSON/JSONC for every client but
Codex (`src/install/json_splice.rs`), TOML for Codex
(`src/install/toml_splice.rs`, built on `toml_edit`). Uninstall removes
only that entry, never the file, through the same format-specific
splice engine (dispatched on `Vendor::mcp_config_format`). Integrity is
judged **semantically** on the entry value (canonical sorted-key JSON
hash — even for the TOML target, whose entry is converted to JSON before
hashing), so reformatting the config is not a modification. Keys a
vendor writes into grim's own member (`Vendor::mcp_entry_vendor_owned_keys`
— Cline's `autoApprove`, `oauth` tokens, …; empty for everyone else) are
left out of that hash and of the adopt/refuse comparison, and every rewrite
carries them over from the member on disk; uninstall still removes the
whole member. Per-client
config files:

| Client | Project | Global |
|--------|---------|--------|
| **Claude** | `<workspace>/.mcp.json` (`mcpServers`) | `~/.claude.json` — `$CLAUDE_CONFIG_DIR/.claude.json` when set (`mcpServers`) |
| **OpenCode** | `<workspace>/opencode.json`/`.jsonc` (`mcp`) | `$OPENCODE_CONFIG` else XDG `opencode.json` (`mcp`) |
| **Copilot** | **two files, two outputs**: `<workspace>/.vscode/mcp.json` (`servers`, `${env:VAR}`) for VS Code's Copilot Chat, then `<workspace>/.github/mcp.json` (`mcpServers`, the global shape, `${VAR}` verbatim) for the Copilot **CLI**, which never reads the VS Code file — `Vendor::mcp_config_paths` + `mcp_entry_for`; a workspace `.mcp.json` (Claude's) hides `.github/mcp.json` from the CLI entirely (live-verified 1.0.88). oauth: `client_id` only → `oauth.clientId` / `oauthClientId` | `$COPILOT_HOME`\|`~/.copilot`/`mcp-config.json` (`mcpServers`); `${VAR}` written verbatim — Copilot CLI expands it (skipped before 2026-09-27) |
| **Codex** | `<workspace>/.codex/config.toml` (`mcp_servers`); only honored by Codex for **trusted** projects — grim writes it regardless, an untrusted project simply won't have it read | `$CODEX_HOME`\|`~/.codex`/`config.toml` (`mcp_servers`); HTTP/SSE headers map to `http_headers`/`env_http_headers`/`bearer_token_env_var`; a header embedding an env ref in text is unrepresentable → descriptor skipped; `timeout` (ms) → `startup_timeout_ms` |
| **Cursor** | `<workspace>/.cursor/mcp.json` (`mcpServers`); stdio needs `type: "stdio"`, env refs `${env:VAR}` | `~/.cursor/mcp.json` (`mcpServers`) |
| **Kiro** | `<workspace>/.kiro/settings/mcp.json` (`mcpServers`); `${VAR}` env refs native | `~/.kiro/settings/mcp.json` (`mcpServers`) |
| **Junie** | `<workspace>/.junie/mcp/mcp.json` (`mcpServers`); env-ref descriptors skipped (interpolation undocumented) | `$JUNIE_HOME`\|`~/.junie`/`mcp/mcp.json` (`mcpServers`) |
| **Gemini** | `<workspace>/.gemini/settings.json` (`mcpServers`); sse → `url`, http → `httpUrl`, `${VAR}` native; `timeout` dropped + warned (Gemini bounds every tool call with it; projected before 2026-09-27) | `~/.gemini/settings.json` (`mcpServers`) |
| **Zed** | `<workspace>/.zed/settings.json` (`context_servers`, flat shape); env-ref descriptors skipped (no upstream support) | `$XDG_CONFIG_HOME`\|`~/.config/zed` (unix) or `%APPDATA%\Zed` (Windows) `/settings.json` (`context_servers`, JSONC) |
| **Amp** | `<workspace>/.amp/settings.json` (`amp.mcpServers`, literal dotted key); `${VAR}` refs | `$XDG_CONFIG_HOME`\|`~/.config/amp`/`settings.json` (`amp.mcpServers`) |
| **Warp** | `<workspace>/.warp/.mcp.json` (`mcpServers`); stdio `command`/`args`/`env`/`working_directory` (from `cwd`), http + sse `url`/`headers`; descriptors with an env ref or any oauth field skipped (expansion undocumented, no oauth keys). Warp also reads Claude's `.mcp.json`/`~/.claude.json` and `.agents/.mcp.json` — double registration when both clients are selected | `~/.warp/.mcp.json` (`mcpServers`) |
| **Droid** | `<workspace>/.factory/mcp.json` (`mcpServers`); `type` always written; `disabled`/`disabledTools` vendor-owned (unhashed, carried over); `${VAR}` verbatim in `env`/`headers`/`oauth.clientId` (Droid expands only there) — a ref in `command`/`args`/`url` skips; oauth `client_id` only, else skip | `~/.factory/mcp.json` (`mcpServers`); Droid's UI copies a toggled project server here, so a same-named global install meets the untracked gate (65) |
| **Qoder** | `<workspace>/.qoder/settings.json` (`mcpServers`) — **not** the shared `.mcp.json` Qoder also reads (Claude's grim-managed file; one member, two vendors, two state outputs); Claude shape + `cwd`, `timeout` ms; env-ref descriptors skipped (expansion undocumented) | `$QODER_CONFIG_DIR`\|`~/.qoder`/`settings.json` (`mcpServers`) |
| **Cline** | — none upstream; `mcp_config_path` is `None`, so a project install warns + skips | `$CLINE_MCP_SETTINGS_PATH`, else `$CLINE_DATA_DIR/settings/`, else `$CLINE_DIR`\|`~/.cline` + `/data/settings/`, file `cline_mcp_settings.json` (`mcpServers`, flat, `${env:VAR}`) — shared by Cline CLI and extension; every splice (install and removal) holds Cline's own `<file>.lock` directory lock (`cline_lock.rs`) and re-reads under it; a held lock past 12 s → exit 75; anchors at `cline-root`, so a file relocated outside `~/.cline` is unanchorable → skipped |

Every non-Claude client declines the `ws` transport (skip + warn). The
structured `oauth` block is lossless-or-skip (`McpOAuth::unmapped`,
`adr_mcp_oauth_projection.md`): OpenCode, Zed, Copilot and Droid write it when they
carry every field it sets (Copilot and Droid: `client_id` only), every other client
skips it (Qoder documents both, in shapes grim cannot carry) — see
`docs/src/content/docs/clients.md` "Known gaps".


### Agents {#install-layout-agents}

An **agent** materializes as a single file in the client's agents
directory (no support directory). For Claude/OpenCode/Copilot/Cursor/Gemini/Qoder
it is a Markdown file (Claude and Qoder install a plain agent verbatim; the others
project the frontmatter, each lifting its own `<vendor>.*` field registry —
e.g. `cursor.readonly`, `gemini.temperature`). For **Codex** it is a
**TOML** file (`<name>.toml`) — Codex is the only TOML-emitting vendor: the
canonical `name`/`description` plus the agent body (as
`developer_instructions`) and an optional `model` are serialized to TOML;
the `tools` field has no Codex equivalent and is dropped with a warning.
For **Antigravity** it is a Markdown file too, with `tools` emitted as a
YAML sequence (upstream types it `string[]`) and nothing lifted — the
`antigravity.*` registry is empty. **Goose** writes `name`/`description`/`model`
only (`tools` dropped + warning, empty `goose.*` registry) into its own
`.goose/agents/`, never the `.agents/agents/` Antigravity owns. **Kilo** takes
OpenCode's shape — filename is the identity, `name` and `tools` dropped,
`kilo.*` lifted — and the same install-time `color`/`steps` drop as OpenCode
(`drop_invalid_opencode_values`; Kilo skips just the invalid agent, OpenCode its
whole config). **Junie** is Markdown with `tools` as a
YAML sequence and `junie.*` lifted to camelCase keys; an agent name outside
Junie's `[a-z][a-z0-9_-]*` is skipped for Junie only. **Droid** writes a
custom droid at `.factory/droids/<name>.md`: `tools` as a YAML list, `model`
verbatim (`inherit` included), `droid.reasoning-effort` → `reasoningEffort`;
a name outside Droid's `[a-z0-9_-]+` (a `.`) is skipped for Droid only. Both
skips go through `Vendor::agent_name_grammar`, checked by
`installer::client_hosts`, so they never count as expected output.
**Every other client declines agents**
(`kind_support == Declined`): CLI/IDE schema collision (Kiro);
Cline's CLI surface documents an installable
format grim does not render yet; ACP/runtime-only (Zed, Amp, OpenClaw); or
no installable format at all (Warp and the generic `agents` target) —
installer skips silently and records no output, logging at
`debug` only.

Per-client agent paths:

| Client | Global agent path |
|--------|-------------------|
| **Claude** | `<claude_root>/agents/<name>.md` |
| **Copilot** | `<copilot_root>/agents/<name>.md` |
| **OpenCode** | `<opencode_root>/agents/<name>.md` |
| **Codex** | `<codex_root>/agents/<name>.toml` |
| **Cursor** | `~/.cursor/agents/<name>.md` (project `.cursor/agents/`) |
| **Gemini** | `<gemini_root>/agents/<name>.md` (project `.gemini/agents/`) |
| **Antigravity** | `~/.gemini/config/agents/<name>.md` (project `.agents/agents/`) |
| **Qoder** | `<qoder_root>/agents/<name>.md` (project `.qoder/agents/`) |
| **Goose** | `~/.goose/agents/<name>.md` (project `.goose/agents/`; `$GOOSE_PATH_ROOT` does not move it) |
| **Kilo** | `$XDG_CONFIG_HOME\|~/.config/kilo/agents/<name>.md` (project `.kilo/agents/`) — **not** the `~/.kilo` skills root |
| **Junie** | `~/.junie/agents/<name>.md` (project `.junie/agents/`, never the shared `.agents/`) |
| **Droid** | `~/.factory/droids/<name>.md` (project `.factory/droids/`) |
| **everyone else** | declined — no agent surface |

`opencode_root` is the parent of the OpenCode skills directory (i.e. the
directory one level above the `skills/` subdir resolved from
`$OPENCODE_CONFIG_DIR` or the XDG default). `claude_root`, `copilot_root`,
`codex_root`, `gemini_root` and `qoder_root` are the vendor roots in the
[`VendorRoot` table](./subsystem-path-anchors.md#path-anchor-set) — `$CODEX_HOME` else `~/.codex`,
`$GEMINI_CLI_HOME/.gemini` else `~/.gemini`, and `$QODER_CONFIG_DIR` else
`~/.qoder`.
