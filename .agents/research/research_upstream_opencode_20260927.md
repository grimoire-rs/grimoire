# Research: upstream refresh — opencode (deep)

## Metadata
**Date:** 2026-09-27 · **Domain:** vendors · **Depth:** `opencode deep named harness`
**Triggered by:** /upstream-refresh opencode (WP-E, plan_harness_capability_freshness)
**Expires:** 2027-03-27

Vendor version at check: OpenCode **1.18.32** (GH release `v1.18.32`,
2026-09-21; npm `opencode-ai` 1.18.32). The repo moved from `sst/opencode`
to `anomalyco/opencode`; sources are pinned to tag `v1.18.32`. The docs
site mirrors `packages/web/src/content/docs/*.mdx` in that repo. First
pass on this row, so the cursor was `—`. The changelog was scanned over 29
`v*` releases from 2026-07-17 (the newest `verified` date among the
OpenCode rows) to 2026-09-21. It showed MCP OAuth and SSE reconnect fixes,
a skills-docs path fix and permission-metadata fixes, and no schema
change. Feed cursor set to `v1.18.32`.

## Claims

Researcher (sonnet) table. Numbered rows re-verify ledger claims; `I` rows
are deep-inventory differences. Quotes are verbatim. URLs under `…/docs/`
are `https://github.com/anomalyco/opencode/blob/v1.18.32/packages/web/src/content/docs/`.

| # | Claim | Ledger location | Old value | New value | Source URL | Verbatim quote | Vendor version |
|---|---|---|---|---|---|---|---|
| 1 | Skill frontmatter fields | watchlist skill-frontmatter row | empty registry, no vendor keys | unchanged | …/docs/skills.mdx | "Only these fields are recognized: - `name` (required) - `description` (required) - `license` (optional) - `compatibility` (optional) - `metadata` (optional, string-to-string map). Unknown frontmatter fields are ignored." | 1.18.32 |
| 2 | Agent `permission` map | watchlist `permission` row | shipped, object-valued | unchanged | …/docs/agents.mdx | "`read`, `edit`, `glob`, `grep`, `list`, `bash`, `task`, `external_directory`, `lsp`, and `skill` accept either a shorthand action … or an object of glob/pattern → action" | 1.18.32 |
| 3 | MCP `oauth: false` | watchlist `oauth: false` row | shipped | unchanged | …/docs/mcp-servers.mdx | "`oauth` \| Object \| false \| OAuth config object, or `false` to disable OAuth auto-detection." | 1.18.32 |
| 4 | Rule scoping | watchlist rule-scoping row | no per-file scoping | unchanged | …/docs/rules.mdx | "All instruction files are combined with your `AGENTS.md` files." | 1.18.32 |
| 5 | `ws` MCP transport | watchlist `ws` row | not documented | unchanged: `local` and `remote` only | …/docs/mcp-servers.mdx | "Add remote MCP servers by setting `type` to `\"remote\"`" | 1.18.32 |
| 6 | MCP `oauth` block | watchlist `oauth` projection row | no native surface documented | **changed**: a native object exists (see V1, V2) | …/docs/mcp-servers.mdx | "`clientId` \| String \| OAuth client ID… `clientSecret` \| String \| OAuth client secret… `scope` \| String \| OAuth scopes to request during authorization." | 1.18.32 |
| 7 | `OPENCODE_AGENT_FIELDS` keys and literals | `vendor_opencode.rs:36`, `vendor-metadata.md` registry | 9 keys | unchanged; `maxSteps` deprecated for `steps`, `tools` for `permission` | …/docs/agents.mdx | "Use a valid hex color (e.g., `#FF5733`) or theme color: `primary`, `secondary`, `accent`, `success`, `warning`, `error`, `info`." | 1.18.32 |
| 8 | MCP entry schema | `vendor_opencode.rs` `mcp_entry`, `mcp-servers.md` | as stated | unchanged; `timeout` default 5000 ms | …/docs/mcp-servers.mdx | "`cwd` \| String \| \| Working directory for the MCP server process. Relative paths resolve from the workspace." | 1.18.32 |
| 9 | `OPENCODE_CONFIG_DIR` additive, `OPENCODE_CONFIG` a file | `vendor_opencode.rs` `global_skills_root`, AGENTS.md env table | as stated | unchanged | [skill/index.ts](https://github.com/anomalyco/opencode/blob/v1.18.32/packages/opencode/src/skill/index.ts) | `unique([Global.Path.config, ...projectUp, ...homeUp, ...(Flag.OPENCODE_CONFIG_DIR ? [Flag.OPENCODE_CONFIG_DIR] : [])])` | 1.18.32 |
| 10 | Agents dir `agents/` | `vendor_opencode.rs` `global_agents_root` | plural | unchanged; singular also accepted | …/docs/config.mdx | "Singular names (e.g., `agent/`) are also supported for backwards compatibility." | 1.18.32 |
| 11 | Skill discovery paths | `vendor-metadata.md` skill-paths table | `.opencode`, `.claude`, `.agents` skills | unchanged; `.claude` scan can be disabled | …/docs/rules.mdx | "export OPENCODE_DISABLE_CLAUDE_CODE_SKILLS=1 # Disable only .claude/skills" | 1.18.32 |
| 12 | Config files and layering | `opencode_config.rs` module doc, `vendor-metadata.md` registration | as stated | unchanged; `.opencode/opencode.json(c)` is merged too (V5) | …/docs/config.mdx | "3. Custom config (`OPENCODE_CONFIG`) … 4. Project config (`opencode.json` in project)" | 1.18.32 |
| 13 | Matrix row Skill ✓ Rule ◐ Agent ✓ MCP ◐ | `clients.md` | as stated | unchanged | (V4, row 4) | — | 1.18.32 |
| 14 | Live-check verbs and opt-outs | `deep-pass.md` opencode row | `agent list`, `mcp list`; opt-out "none documented" | adds `debug skill`, `debug config`; `OPENCODE_DISABLE_AUTOUPDATE`; no login | [cli/cmd/debug/skill.ts](https://github.com/anomalyco/opencode/blob/v1.18.32/packages/opencode/src/cli/cmd/debug/skill.ts) | `command: "skill", describe: "list all available skills"` | 1.18.32 |
| I1 | OAuth object | — | — | see V1, V2 | [config/mcp.ts](https://github.com/anomalyco/opencode/blob/v1.18.32/packages/core/src/v1/config/mcp.ts) | (verifier) | 1.18.32 |
| I2 | Config layers | — | — | remote `.well-known/opencode` → global → `OPENCODE_CONFIG` → project → `.opencode` → `OPENCODE_CONFIG_DIR` → `OPENCODE_CONFIG_CONTENT` → managed (`/etc/opencode`, macOS MDM) | …/docs/config.mdx | (researcher) | 1.18.32 |
| I3 | Singular or plural subdirs | — | — | every `.opencode` subdir accepts both | …/docs/config.mdx | (row 10) | 1.18.32 |
| I4 | `permission.skill` | — | — | gates skill loading per agent or globally | (researcher, no URL) | — | 1.18.32 |
| I5 | Config-declared skill sources | — | — | `skills.paths`, `skills.urls` in `opencode.json` | (researcher, no URL) | — | 1.18.32 |
| I6 | Org-default MCP | — | — | `.well-known/opencode` ships defaults | (researcher, no URL) | — | 1.18.32 |
| I7 | Plugins and hooks | finding only (PR #98) | — | local `.opencode/plugins/`, npm `plugin: []`; command, file, LSP, message, permission, session, tool, TUI events | (researcher, no URL) | — | 1.18.32 |
| I8 | Built-in skill | — | — | `customize-opencode` registers before disk discovery (live-seen in `debug skill`) | live check | — | 1.18.32 |
| I9 | Other `OPENCODE_*` path flags | AGENTS.md env table | two variables | `flag.ts` also defines `OPENCODE_CONFIG_CONTENT`, `OPENCODE_DISABLE_PROJECT_CONFIG`, `OPENCODE_TUI_CONFIG`, `OPENCODE_PERMISSION`; none moves a path grim writes | [flag.ts](https://github.com/anomalyco/opencode/blob/v1.18.32/packages/core/src/flag/flag.ts) | (names only) | 1.18.32 |

Rows I1–I9 carry no quote of their own. None drives a code change, so they
stand as findings only.

## Verifier sign-off

No claim drives a renderer, metadata or validation change in this pass.
The opus verifier still re-fetched the sources behind the watchlist edits
and the two issue drafts (C-008), at `v1.18.32`.

| Claim | Verdict | Evidence URL |
|---|---|---|
| V1 `oauth` is `object \| false` with `clientId`, `clientSecret`, `scope` | PARTIAL. The object has five optional fields: those three plus `callbackPort` (1–65535) and `redirectUri`, which the docs omit. There is no metadata-URL field. It is remote-only, and `{env:VAR}` works in it, because substitution runs on the raw text | https://github.com/anomalyco/opencode/blob/v1.18.32/packages/core/src/v1/config/mcp.ts |
| V1a `scope` is space-separated | CONFIRMED | https://github.com/anomalyco/opencode/blob/v1.18.32/packages/opencode/src/mcp/oauth-provider.ts |
| V2 targets for grim's `McpOAuth` | `client_id` → `clientId`, `scopes` → `scope` (space-joined), `callback_port` → `callbackPort` (default 19876). `auth_server_metadata_url` has no target, but OpenCode auto-discovers the server (RFC 9728, then RFC 8414), so dropping it fails only against servers that publish no discovery metadata | https://github.com/anomalyco/opencode/blob/v1.18.32/packages/opencode/src/mcp/index.ts |
| V3 agent key types | PARTIAL. All 9 keys have the stated types. `color` must match `^#[0-9a-fA-F]{6}$` or be one of 7 theme names. `steps` is a positive integer. `temperature` and `top_p` are finite, with no range check. Unknown keys fold into `options` | https://github.com/anomalyco/opencode/blob/v1.18.32/packages/core/src/v1/config/agent.ts |
| V3a one bad agent value fails the whole config | CONFIRMED. Legacy `mode/*.md` files are the exception, and are skipped silently | https://github.com/anomalyco/opencode/blob/v1.18.32/packages/opencode/src/config/parse.ts |
| V4 MCP local and remote keys | CONFIRMED. Only `local` and `remote` exist, and `remote` falls back to SSE internally. Unknown keys are ignored, not rejected | https://github.com/anomalyco/opencode/blob/v1.18.32/packages/core/src/v1/config/mcp.ts |
| V5 config layering | CONFIRMED. `.opencode/` and `$OPENCODE_CONFIG_DIR` files merge after the project root. `OPENCODE_CONFIG` loads before the project files. At the project root, `.json` merges first and `.jsonc` wins, which matches grim editing the `.jsonc` when it exists | https://github.com/anomalyco/opencode/blob/v1.18.32/packages/opencode/src/config/paths.ts |

## Live CLI check

```text
opencode agent list (native linux-x64 binary, env -i, sandbox HOME, OPENCODE_CONFIG_DIR, TMPDIR, DO_NOT_TRACK, OPENCODE_DISABLE_AUTOUPDATE, OPENCODE_DISABLE_MODELS_FETCH) — pass — grim's agent listed as subagent, project and global scope
opencode debug agent agent — pass — mode, temperature, steps, color and model read back as rendered
opencode debug skill — pass — grim-usage listed from .opencode/skills (project) and $OPENCODE_CONFIG_DIR/skills (global)
opencode debug config — pass — managed instructions glob present (relative at project scope, absolute $GRIM_HOME glob at global scope); both MCP entries parsed; {env:VAR} header resolved
opencode mcp list, grim on PATH — pass — grim stdio entry connected; the fixture remote URL (.invalid host) failed to connect, as expected
opencode mcp list, OPENCODE_CONFIG=<commented .jsonc> — pass — grim spliced the entry, the comment survived, and OpenCode read it
opencode mcp list, existing global ~/.config/opencode/opencode.jsonc — pass — grim edited the .jsonc in place, created no .json
negative control: agent with mode: pilot — fail as intended — every verb exits 1 with "Configuration is invalid"
rules (content load) — skip — no headless verb shows loaded instruction text; debug config confirms only the glob
```

No login was needed. The npm stub `bin/opencode.exe` refuses to run
without its postinstall script, so the check ran the native binary that
the optional dependency ships, and no install script executed. Without
`TMPDIR` the CLI created `/tmp/opencode` outside the sandbox (removed).
Sandbox and registry container removed.

## Routing

| Claim | Class | Landed as |
|---|---|---|
| 1, 2, 3, 4, 5 | (a) | watchlist rows dated; shared OpenCode+Copilot rows split so OpenCode's date cannot freshen Copilot's |
| 6 / V1 / V2 OAuth | (a) + (c) | watchlist row and `mcp-servers.md` text updated; issue draft 1 |
| 7 / V3 color, steps | (a) + (c) | `vendor-metadata.md` registry rows and a new watchlist row; issue draft 2 |
| 8, 10, 12, 13, V4, V5 | (a) | confirmed unchanged; `vendor_opencode.rs` module doc dated |
| 9 | (a) | `OPENCODE_CONFIG_DIR` and `OPENCODE_CONFIG` rows in the env-var table (C-018) |
| 11, I3, I9 | none | no path grim writes moves |
| 14 | (a) | `deep-pass.md` and `domains.md` opencode rows — skill commit |
| I2, I4, I5, I6, I8 | finding | no grim surface |
| I7 hooks | finding | out of scope (PR #98) |
| catalog drift | none | `catalog/**` holds no OpenCode oauth, color or steps claim; `mcp-spec.md` still reads "oauth: Claude only", which stays true |

Security: none found.

No renderer or metadata diff landed, so no `reviewer:spec` ran and no
self-heal test was added. The commits change docs text, watchlist rows
and one module doc comment.

**Overlap with `fix/opencode-global-jsonc`.** The sibling branch
(`a8da7dce`, worktree `grimoire-wt-opencode-jsonc`) edits an existing
global `opencode.jsonc` in place. Main already does this
(`opencode_config::global_config_path`, test
`global_config_prefers_existing_jsonc`), and this pass live-verified it:
an existing `~/.config/opencode/opencode.jsonc` was edited in place, and
no `.json` was created. V5 confirms `.jsonc` is the right file, because it
merges last. The sibling branch was not touched. Its owner should check
whether it is still needed. Its docs hunks target the pre-Starlight paths
(`docs/src/*.md`).

## Issue drafts

Filed as https://github.com/grimoire-rs/grimoire/issues/152.

### 1. OpenCode: project `[server.oauth]` onto OpenCode's `oauth` object?

**Labels:** `enhancement`, `vendor:opencode`, `needs-design`

OpenCode 1.18.32 has a native `oauth` object on remote MCP entries
([config/mcp.ts](https://github.com/anomalyco/opencode/blob/v1.18.32/packages/core/src/v1/config/mcp.ts)).
It takes `clientId`, `clientSecret`, `scope` (one space-separated
string), `callbackPort` (default 19876) and `redirectUri`, or `false` to
disable auto-detection. grim skips any descriptor carrying
`[server.oauth]` for OpenCode today. Three of the four `McpOAuth` fields
map directly: `client_id`, `scopes` (space-joined) and `callback_port`.
`auth_server_metadata_url` has no target, but OpenCode discovers the
authorization server itself (RFC 9728, then RFC 8414), so dropping it
fails only against a server that publishes no discovery metadata.
Decide:

- whether to project the block, dropping the metadata URL with a warning;
- or to project it only when that field is absent.

Either way the change turns a skip into an install for descriptors that
were skipped before, so the upgrade note must say so. `callbackPort` and
`redirectUri` are in the source schema but not in OpenCode's docs.

### 2. OpenCode: agent `color` and `steps` values that OpenCode rejects break its whole config

**Labels:** `bug`, `vendor:opencode`, `needs-design`

`opencode.color` is a free `String` and `opencode.steps` a free
`Integer` in `OPENCODE_AGENT_FIELDS`. OpenCode 1.18.32 accepts only
`^#[0-9a-fA-F]{6}$` or `primary|secondary|accent|success|warning|error|info`
for `color`, and a positive integer for `steps`
([config/agent.ts](https://github.com/anomalyco/opencode/blob/v1.18.32/packages/core/src/v1/config/agent.ts)).
One bad value makes OpenCode reject its whole config. A live check with
`mode: pilot` made every verb exit 1 with "Configuration is invalid". An
artifact with `color: "#FFF"` therefore installs cleanly and then breaks
OpenCode for every agent in that scope. Two options:

- Tighten publish-time validation, which rejects artifacts accepted today
  and is a compatibility change.
- Drop the invalid value at render time with a warning, which is a
  class-1 repair of grim's own output.

The docs now state the constraint (`vendor-metadata.md` › opencode.*
agent registry).

## Friction

- **The deep-pass opencode note was "docs-only, never probed."** The live
  run found four no-login verbs, including `debug skill` and
  `debug config`. It also found the npm stub, the `TMPDIR` leak to
  `/tmp/opencode`, and that `mcp list` dials servers, so grim must be on
  `PATH` for a `connected` signal. Fixed in `deep-pass.md` and
  `domains.md`.
- **Telemetry cell said "none documented (only the `autoupdate` config
  key)".** It now names `OPENCODE_DISABLE_AUTOUPDATE` and
  `OPENCODE_DISABLE_MODELS_FETCH`. `flag.ts` at v1.18.32 defines no
  telemetry variable.
- **zsh does not word-split** an unquoted `$c` holding `agent list`, so
  OpenCode took it as a project path ("Failed to change directory"). This
  is the same class as the codex `-c` flag note. The recipe says so, and
  the check ran from a bash script.
- **Researcher claims 6 and 7 understated the source.** The docs table
  omits `callbackPort` and `redirectUri`, and the color and steps
  constraints appear only in source. The verifier caught both. No brief
  change: the brief already asks for primary sources, and the verifier
  gate did its job.
- Two OpenCode rows were shared with Copilot; they were split, as the
  codex pass did.
- No researcher retries, no unsourced claims.

## Skill changes

A `chore(skills)` commit follows this record. It changes the
`deep-pass.md` opencode row (native binary, `TMPDIR`, autoupdate and
models-fetch opt-outs, `debug skill` and `debug config` verbs) and the
opencode note (no login, both scopes, grim on `PATH` for `mcp list`,
strict config validation, zsh word-splitting). It also updates the
`domains.md` opencode live-check cell.
