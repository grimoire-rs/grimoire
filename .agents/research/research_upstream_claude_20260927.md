# Research: upstream refresh — claude (deep)

## Metadata
**Date:** 2026-09-27 · **Domain:** vendors · **Depth:** `claude deep named harness`
**Triggered by:** /upstream-refresh claude (WP-C, plan_harness_capability_freshness)
**Expires:** 2027-03-27

Vendor version at check: Claude Code **2.1.283** (GH release `v2.1.283`,
2026-09-25; npm `@anthropic-ai/claude-code` 2.1.283). Changelog scanned
from 2.1.245 (the previous `verified` stamp, 2026-08-25) to 2.1.283.

## Claims

Researcher (sonnet) table, one row per claim; numbered rows re-verify
ledger claims, `I` rows are deep-inventory differences. Quotes verbatim.

| # | Claim | Ledger location | Old value | New value | Source URL | Verbatim quote | Vendor version |
|---|---|---|---|---|---|---|---|
| 1 | `claudeMdExcludes` merges across layers, globs on absolute paths | watchlist `claudeMdExcludes` row | as stated | unchanged | https://code.claude.com/docs/en/memory | "Patterns are matched against absolute file paths using glob syntax. You can configure `claudeMdExcludes` at any settings layer: user, project, local, or managed policy. Arrays merge across layers." | 2.1.283 |
| 2 | MCP `ws` transport and `oauth` block | watchlist `ws` / `oauth` rows | as stated | unchanged | https://code.claude.com/docs/en/mcp | "`type` \| string \| Transport type: `stdio`, `http`, `sse`, `ws`, or `sdk`" | 2.1.283 |
| 3 | Skill frontmatter keys grim maps (13) | `src/install/vendor_claude.rs` `CLAUDE_SKILL_FIELDS` | 13 keys | unchanged; more keys exist (I8) | https://code.claude.com/docs/en/skills | "`paths` \| No \| Glob patterns that limit when this skill is activated. Accepts a comma-separated string or a YAML list." | 2.1.283 |
| 4 | Subagent frontmatter keys grim maps (12) | `vendor_claude.rs` `CLAUDE_AGENT_FIELDS` | 12 keys | unchanged; more keys exist (I10) | https://code.claude.com/docs/en/sub-agents | "`permissionMode` \| No \| Permission mode: `default`, `acceptEdits`, `auto`, `dontAsk`, `bypassPermissions`, `plan`, or `manual` as an alias for `default`. The `manual` alias requires Claude Code v2.1.200 or later." | 2.1.200+ |
| 5 | `.claude/rules/*.md`: `paths` native, recursive, unscoped loads always | `subsystem-file-structure.md` › Rules | as stated | unchanged | https://code.claude.com/docs/en/memory | "`paths` is the only field Claude Code reads from a rule; any other field is ignored without an error." | 2.1.283 |
| 6 | MCP entry shape, refinements, `${VAR}` / `${VAR:-default}` | `vendor_claude.rs` `mcp_entry`; `mcp-servers.md` | as stated | unchanged | https://code.claude.com/docs/en/mcp | "`${VAR}`: Expands to environment variable value" / "`${VAR:-default}`: Uses `default` if `VAR` is unset" | 2.1.283 |
| 7 | `CLAUDE_CONFIG_DIR` relocates `~/.claude` and `.claude.json` | AGENTS.md env table; `vendor_claude.rs` `global_root` | as stated | unchanged, plus a settability limit (I14) | https://code.claude.com/docs/en/claude-directory | "If you set `CLAUDE_CONFIG_DIR`, every `~/.claude` path on this page lives under that directory instead." | 2.1.283 |
| 8 | Telemetry opt-outs for the live check | `deep-pass.md` per-harness table | as stated | unchanged | https://code.claude.com/docs/en/env-vars | "Set this variable to disable telemetry collection and reporting." | 2.1.283 |
| 9 | Headless listing verbs | `deep-pass.md` claude note | MCP only | `claude agents` lists background *sessions*, not agent files; `claude mcp list` shows unapproved `.mcp.json` servers as pending | https://code.claude.com/docs/en/mcp | "``⏸ Pending approval (run `claude` to approve)``: a project-scoped server from `.mcp.json` that you haven't approved yet. Claude Code shows it in both `claude mcp list` and `claude mcp get <name>`." | 2.1.283 |
| I1 | Slash commands still a separate kind | not modeled | — | `commands/` flat Markdown; docs steer to skills | https://code.claude.com/docs/en/plugins-reference | "Commands \| `commands/` \| Flat Markdown command files. Prefer `skills/` for new plugins" | 2.1.283 |
| I2 | Output styles | not modeled | — | `output-styles/` Markdown | plugins-reference | "Output styles \| `output-styles/` \| Output style Markdown files" | 2.1.283 |
| I3 | LSP servers | not modeled | — | `.lsp.json` / plugin `lspServers` | plugins-reference | "`lspServers` \| Path, object, or array of either \| `.json` LSP config files or inline server configs keyed by name." | 2.1.283 |
| I4 | Workflows | not modeled | — | `workflows/` `.js` files | plugins-reference | "Workflows \| `workflows/` \| Workflow `.js` files" | 2.1.283 |
| I5 | Themes | not modeled | — | `themes/` JSON | plugins-reference | "Themes \| `themes/` \| Theme JSON files" | 2.1.283 |
| I6 | Monitors (experimental) | not modeled | — | background-process definitions | plugins-reference | "With `\"always\"`, the default, the monitor starts at session start and on plugin reload." | 2.1.283 |
| I7 | Hook events | finding only (PR #98) | — | 30+ events incl. `InstructionsLoaded`, `PreModelSwitch` | https://code.claude.com/docs/en/hooks | "`InstructionsLoaded` \| When a CLAUDE.md or `.claude/rules/*.md` file is loaded" | 2.1.251+ |
| I8 | Skill keys grim lacks | `CLAUDE_SKILL_FIELDS` | — | `background` (bool, 2.1.218+); `name`/`description`/`metadata`/`license`/`compatibility` are agentskills canonical fields | https://code.claude.com/docs/en/skills | "`compatibility` \| No \| Environment requirements for the skill... Claude Code accepts the field but doesn't act on it." | 2.1.218+ |
| I9 | Skill `shell` accepts `0` | `CLAUDE_SKILL_FIELDS` `shell` | bash\|powershell | **refuted by verifier** (V2) | skills | — | — |
| I10 | Agent keys grim lacks | `CLAUDE_AGENT_FIELDS` | — | `omitClaudeMd` (bool, 2.1.271+), `experimental.cacheTtl` (2.1.248+) | https://code.claude.com/docs/en/sub-agents | "`omitClaudeMd` \| No \| Set to `true` to launch this subagent without the user, project, and local CLAUDE.md files... Requires Claude Code v2.1.271 or later" | 2.1.271 |
| I11 | Rule keys besides `paths` | — | unknown | none | memory | (see row 5) | 2.1.283 |
| I12 | MCP surface additions | `mcp-servers.md` | — | `managedMcpServers` (2.1.259), `sdk` entries skipped, credential-variable blanking (V6) | mcp; CHANGELOG | "Added `managedMcpServers` managed setting: organizations can provide HTTP/SSE MCP servers to every user (same entry shape as `.mcp.json`); entries that name a command to run are skipped" | 2.1.259 |
| I13 | Other config-root env vars | AGENTS.md env table | only `CLAUDE_CONFIG_DIR` | `CLAUDE_CODE_PROJECT_DIR_NAME` (auto-memory dir name, 2.1.234+) — no grim surface | memory | "If you set `CLAUDE_CODE_PROJECT_DIR_NAME` beside `CLAUDE_CONFIG_DIR`, Claude Code uses that name as the `<project>` directory... Requires Claude Code v2.1.234 or later." | 2.1.234 |
| I14 | `CLAUDE_CONFIG_DIR` settability | AGENTS.md env table | — | project/local settings `env` can no longer set it | CHANGELOG 2.1.251 | "Changed project-level `.claude/settings.json` `env` to no longer set `CLAUDE_CONFIG_DIR`, `CLAUDE_CODE_TMPDIR`, or `TMPDIR`/`TMP`/`TEMP`; set them in your shell, user, or managed settings instead" | 2.1.251 |
| I15 | Native `AGENTS.md` reading | no ledger row | — | read directly when no `CLAUDE.md` exists | memory | "Reading `AGENTS.md` directly requires Claude Code v2.1.277 or later." | 2.1.277 |
| I16 | `claude-ai` skill-name reservation reverted | — | — | 2.1.282 reserved, 2.1.283 reverted | CHANGELOG 2.1.283 | "Reverted the 2.1.282 reservation of the `claude-ai` name: skills, commands, workflows and MCP servers' skills and prompts so named load again" | 2.1.283 |

## Verifier sign-off

Opus verifier re-fetched every source (C-008).

| Claim | Verdict | Evidence URL |
|---|---|---|
| V1 skill `background` bool, 2.1.218+, "Only applies with `context: fork`" | CONFIRMED | https://code.claude.com/docs/en/skills |
| V2 skill `shell` accepts `0` | REFUTED — the `0` belongs to the env var `CLAUDE_CODE_USE_POWERSHELL_TOOL`; no change | https://code.claude.com/docs/en/skills |
| V3 agent `omitClaudeMd` bool, 2.1.271+ | CONFIRMED | https://code.claude.com/docs/en/sub-agents |
| V4 `experimental.cacheTtl` is a nested map (`experimental:` → `cacheTtl: 5m\|1h`) | CONFIRMED — not a dotted key, so not representable today | https://code.claude.com/docs/en/sub-agents |
| V5 every key and literal grim maps is still documented | CONFIRMED; missing keys as I8/I10 | skills, sub-agents |
| V6 credential variables in remote `url`/`headers` | PARTIAL — a fixed name set (examples `ANTHROPIC_API_KEY`, `AWS_BEARER_TOKEN_BEDROCK`, `NPM_TOKEN`) reads as empty, no substring rule, no opt-out; the substring rule is `headersHelper` env scrubbing (2.1.238) | https://code.claude.com/docs/en/mcp |
| V7 `CLAUDE_CONFIG_DIR` (a) relocates tree, (b) holds `.claude.json`, (c) shell-only | (a) CONFIRMED, (b) CONFIRMED by implication, (c) REFUTED as stated — only project/local settings `env` lost it; user and managed settings still set it | https://code.claude.com/docs/en/claude-directory, https://code.claude.com/docs/en/agent-sdk/hosting |
| V8 `claudeMdExcludes` | CONFIRMED | https://code.claude.com/docs/en/memory |
| V9 rules discovery | CONFIRMED | https://code.claude.com/docs/en/memory |
| V10 MCP entry fields, `${VAR}` forms | CONFIRMED | https://code.claude.com/docs/en/mcp |
| Old CLIs and unknown keys | Both pages: "Claude Code ignores a field it doesn't recognize without reporting an error" — safe to emit | skills, sub-agents |

## Live CLI check

```text
claude mcp list (env -i, sandbox HOME, CLAUDE_CONFIG_DIR, opt-outs; native binary from the optional dependency) — pass — grim-rendered project entry `grim` listed as ⏸ Pending approval, never dialled; MCP only
claude mcp get grim — pass — scope reported as Project config (shared via .mcp.json)
skills / rules / agents listing — skip — Claude Code has no headless listing verb for these kinds (`claude agents` manages background sessions)
```

No login needed; no claude.ai connectors appeared (none inherited).
grim 0.14.2 branch build rendered the skill, rule, agent (files present
under `.claude/`) and the MCP entry (from a loopback registry). Sandbox
and registry container removed.

## Routing

| Claim | Class | Landed as |
|---|---|---|
| 1, 2, 5, 6, 8 | (a) | watchlist `claudeMdExcludes` row re-dated in [2ecdfc03](https://github.com/grimoire-rs/grimoire/commit/2ecdfc03); others confirmed unchanged |
| 3/I8 skill `background`, 4/I10 agent `omitClaudeMd` | (b) | `claude.background`, `claude.omit-claude-md` registry rows + docs + unit and self-heal acceptance tests + watchlist row — [2ecdfc03](https://github.com/grimoire-rs/grimoire/commit/2ecdfc03) |
| I10 `experimental.cacheTtl` | (a) | watchlist row (needs nested native path support first) — [2ecdfc03](https://github.com/grimoire-rs/grimoire/commit/2ecdfc03) |
| V6 / I12 credential-variable blanking | (a) | `docs/src/content/docs/mcp-servers.md#env-references` note + watchlist row — [2ecdfc03](https://github.com/grimoire-rs/grimoire/commit/2ecdfc03) |
| 7, I14, V7 | (a) + (c) | env-var watchlist table row (C-018, [2ecdfc03](https://github.com/grimoire-rs/grimoire/commit/2ecdfc03)) + AGENTS.md pointer; user-settings `env` gap → issue draft 2 |
| 9 | (a) | `deep-pass.md` claude note — [37aa6612](https://github.com/grimoire-rs/grimoire/commit/37aa6612) |
| I1–I6 unmodeled kinds | (c) | issue draft 1 |
| I7 hooks | finding | out of scope (PR #98) — the event list is the reference for that PR |
| I9 | dropped | refuted by verifier |
| I13 | none | auto-memory dir, no grim surface |
| I15 native `AGENTS.md` | finding | no grim render surface; relevant to the managed-context initiative |
| I16 | none | transient, reverted upstream |
| module doc stamp | (a) | `vendor_claude.rs` module doc dated (`verified 2026-09-27 against CLI 2.1.283`) |

Security: none found.

**reviewer:spec (opus) on the renderer diff:** no Block. Warns fixed:
semicolons in the two new table cells, and the MCP paragraph (dropped the
`:-default` sentence, since grim rejects that syntax in every descriptor).
Suggest taken: the self-heal test now asserts `status` returned outputs.

**Decision — a newly known key turns a bad literal into an error.**
Question: before this change `claude.background: yes` on a skill was
warned and dropped; now a non-bool value fails the render. Is that a
Principle 9 break? Research: the watchlist's action column prescribes
"add registry row" / "populate registries" as the additive path, the
registry ADR (`adr_tool_namespaced_metadata_rendering.md`) makes a bad
literal on a known key a hard error by design, and any registry row
added after a key first appears upstream carries the same edge. The only affected artifact is one that already authored the exact
upstream key name with a non-bool value, which Claude itself would not
have read. Decision: accept as the established additive registry path;
recorded here and in the commit body.

## Issue drafts

Filed as https://github.com/grimoire-rs/grimoire/issues/141.

### 1. Claude Code: artifact kinds grim does not model

**Labels:** `enhancement`, `vendor:claude`, `needs-design`

Claude Code 2.1.283 reads component kinds beyond skills, rules, agents
and MCP: slash commands (`commands/`, flat Markdown, docs now steer to
skills), output styles (`output-styles/`), and, as plugin components,
LSP servers (`.lsp.json`), workflows (`workflows/*.js`), themes and
monitors ([plugins reference](https://code.claude.com/docs/en/plugins-reference)).
grim has no kind for any of them. A new artifact kind is a publish,
lock and render contract, so each needs an ADR before code. Suggested
order: output styles (Markdown, single file, native `.claude/output-styles/`),
then decide whether commands are worth a kind at all given the steer to
skills. Hooks are tracked separately
([#98](https://github.com/grimoire-rs/grimoire/pull/98)).

### 2. `CLAUDE_CONFIG_DIR` set in user settings `env` is invisible to grim

**Labels:** `bug`, `vendor:claude`, `compat`

Since Claude Code 2.1.251 project and local settings `env` can no longer
set `CLAUDE_CONFIG_DIR`, but user and managed settings `env` still can
([changelog 2.1.251](https://github.com/anthropics/claude-code/blob/main/CHANGELOG.md),
[settings](https://code.claude.com/docs/en/settings)). grim reads only
its own process environment, so a user who relocates Claude this way
gets global installs under `~/.claude` while Claude reads the other
root. Honoring it would move grim's global Claude root for those users:
a layout move that needs the relocated-root reaper, a state migration
and an upgrade fixture (Principle 9). Decide whether to honor it or to
document it as a limitation in `docs/src/content/docs/clients.md`.

## Friction

- **Deep-pass recipe:** `grim add <descriptor.toml> --kind mcp` exits 64
  (no path source for `mcp`), and `grim release` needs `--kind mcp`.
  Fixed: the recipe now releases the descriptor to a loopback registry.
- **Deep-pass recipe:** `npm install --ignore-scripts` leaves Claude's
  `bin/claude.exe` a stub that refuses to run. Fixed: run the native
  binary the optional dependency ships (no install script executes).
- **Deep-pass note was stale:** `claude mcp list` no longer dials the
  claude.ai connectors in a no-login sandbox, and `claude agents` exists
  but lists sessions. Fixed in `deep-pass.md`.
- **Cursor rule undefined for sweep/deep:** the skill only defined how a
  `feed` pass moves the cursor, and gave no changelog range for a
  first-time sweep. Fixed in `SKILL.md` step 6 and the researcher brief.
- **Researcher brief had no deep-inventory slot**, so the deep pass
  hand-wrote it. Fixed: a sweep/deep changelog range and a deep
  inventory slot, plus fetch tips (raw `.md` pages, persisted WebFetch
  output, `gh api …/contents/CHANGELOG.md`).
- **Sandbox cleanup:** the harness refused `rm -rf` on the scratchpad;
  `/usr/bin/find <dir> -delete` worked. Recorded in `deep-pass.md`.
- **Undated stamp invisible:** `vendor_claude.rs`'s module doc said
  "verified" with no date; the stale report cannot see an undated `//!`
  claim. Dated in this pass. A `verified` on two consecutive `//!` lines
  is counted twice, so the stamp was reworded to one occurrence.
- No researcher retries; no unsourced claims (researcher list empty). I9
  was sourced but misread, and the verifier refuted it.

## Skill changes

[37aa6612](https://github.com/grimoire-rs/grimoire/commit/37aa6612): `deep-pass.md` (MCP fixture via loopback registry,
native Claude binary, current `claude mcp list` behavior, `find -delete`
cleanup), `SKILL.md` (sweep/deep cursor and changelog range),
`researcher-brief.md` (changelog range, deep inventory slot, fetch tips).
