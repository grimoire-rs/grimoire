# Research: upstream refresh — gemini (sweep)

## Metadata
**Date:** 2026-09-27 · **Domain:** vendors · **Depth:** `gemini sweep never checked`
**Triggered by:** /upstream-refresh --domain vendors (WP-G, plan_harness_capability_freshness)
**Expires:** 2027-03-27

Ladder output for the whole `--domain vendors` run (the four Tier 1 rows
were deep-checked earlier today, so `noop`; the 14 Tier 2 rows `sweep`):

```text
cursor sweep never checked
kiro sweep never checked
junie sweep never checked
gemini sweep never checked
zed sweep never checked
amp sweep never checked
antigravity sweep never checked
cline sweep never checked
droid sweep never checked
goose sweep never checked
warp sweep never checked
openclaw sweep never checked
kilo sweep never checked
qoder sweep never checked
claude noop checked today
opencode noop checked today
copilot noop checked today
codex noop checked today
```

Vendor version at check: Gemini CLI **v0.61.0** (stable, 2026-09-23).
Feed: GH `google-gemini/gemini-cli` releases, stable `v*` only; cursor was
`—`, **v0.61.0** recorded (`-nightly`/`-preview` skipped). Changelog
2026-07-17 → 2026-09-27 (v0.52.0 … v0.59.0 via `docs/changelogs/index.md`):
security and triage work — MCP OAuth SSRF mitigation, fail-closed workspace
trust filtering of `mcpServers` (v0.59.0), symlink handling (v0.58.0). None
changes a shape grim renders.

**Carry-over from WP-D — what does Gemini's MCP `timeout` mean?** Question:
grim writes the descriptor's millisecond `timeout` verbatim; is the unit and
meaning right? Research: claim 8 and V1 — milliseconds, and the one value is
used for the connect and for every `callTool`, default 10 minutes. Decision:
the unit is right, so no render change; the broader meaning is disclosed in
`mcp-servers.md` and drafted as an issue, because dropping or rescaling a key
grim already writes changes existing installs.

## Claims

Researcher (sonnet) table, verbatim. Numbered rows re-verify ledger claims;
`I` rows are changelog or surface differences found in the sweep.

| # | Claim | Ledger location | Old value | New value | Source URL | Verbatim quote | Vendor version |
|---|---|---|---|---|---|---|---|
| 1 | `experimental.enableAgents` default | `.claude/rules/vendor-capability-watchlist.md:136` | default `true` pinned via `settingsSchema.ts` + revert PR #23672 | unchanged | https://github.com/google-gemini/gemini-cli/blob/v0.61.0/packages/cli/src/config/settingsSchema.ts#L2224 | `enableAgents: { type: 'boolean', label: 'Enable Agents', category: 'Experimental', requiresRestart: true, default: true, description: 'Enable local and remote subagents.' }` | v0.61.0 |
| 2 | MCP oauth block shape | `.claude/rules/vendor-capability-watchlist.md:137` | `{enabled}`/`authProviderType` shape ≠ grim's `McpOAuth` | unchanged | https://github.com/google-gemini/gemini-cli/blob/v0.61.0/packages/core/src/mcp/oauth-provider.ts#L44 | `export interface MCPOAuthConfig { enabled?: boolean; // Whether OAuth is enabled for this server clientId?: string; clientSecret?: string; ... }` (separate `authProviderType?: AuthProviderType` field on `MCPServerConfig`) | v0.61.0 |
| 3 | Agent inline `mcpServers` | `.claude/rules/vendor-capability-watchlist.md:138` | agent frontmatter now allows inline `mcpServers` | unchanged | https://github.com/google-gemini/gemini-cli/blob/v0.61.0/docs/core/subagents.md | `\| mcpServers \| object \| No \| Configuration for inline Model Context Protocol (MCP) servers isolated to this specific agent. \|` | v0.61.0 |
| 4 | Pool root vs `GEMINI_CLI_HOME` | `.claude/rules/vendor-capability-watchlist.md:145` | Upstream **does** derive its pool from the overridden homedir (`Storage::getUserAgentSkillsDir()`) | unchanged | https://github.com/google-gemini/gemini-cli/blob/v0.61.0/packages/core/src/utils/paths.ts#L22 | `export function homedir(): string { const envHome = process.env['GEMINI_CLI_HOME']; if (envHome) { return envHome; } return os.homedir(); }` (chained through `Storage.getGlobalAgentsDir()` → `getUserAgentSkillsDir()`) | v0.61.0 |
| 5 | Rules (Antigravity) sunset row | `.claude/rules/vendor-capability-watchlist.md:135` | individual-tier Gemini CLI sunset 2026-06-18 → Antigravity CLI; **done 2026-07-26** | unchanged (already closed; no new upstream fact to record) | https://developers.googleblog.com/an-important-update-transitioning-gemini-cli-to-antigravity-cli/ | — (already-closed record; no fresh source quote sought) | n/a |
| 6 | `GEMINI_CONFIG_DIR` does not exist upstream (FR #2815) | `src/install/vendor_gemini.rs` module doc | `GEMINI_CONFIG_DIR` does not exist upstream (FR #2815) | unchanged | https://github.com/google-gemini/gemini-cli/issues/2815 | `Add GEMINI_CONFIG_DIR environment variable to specify a custom path for the .gemini config directory.` ... `Closing this in favor of #1825` | issue closed as duplicate, not shipped; confirmed absent from source at v0.61.0 |
| 7 | `GEMINI_CLI_HOME` replaces homedir, `.gemini` appended | `src/install/vendor_gemini.rs` module doc; `docs/src/content/docs/vendor-metadata.md:408` | replaces Node's `os.homedir()`; config dir is `$GEMINI_CLI_HOME/.gemini` | unchanged | https://github.com/google-gemini/gemini-cli/blob/v0.61.0/docs/reference/configuration.md | `**GEMINI_CLI_HOME**: Specifies the root directory for Gemini CLI's user-level configuration and storage. By default, this is the user's system home directory. The CLI will create a .gemini folder inside this directory.` | v0.61.0 |
| 8 | CARRY-OVER: MCP `timeout` field unit + meaning | `src/install/vendor_gemini.rs:195` (grim writes descriptor's ms `timeout` verbatim) | (undocumented in grim ledger; carry-over question) | milliseconds; used as BOTH the MCP client connect/handshake timeout AND the per-tool-call (`callTool`) request timeout — same value reused for both, default 600000ms (10 min) | https://github.com/google-gemini/gemini-cli/blob/v0.61.0/packages/core/src/tools/mcp-client.ts ; https://github.com/google-gemini/gemini-cli/blob/v0.61.0/docs/reference/configuration.md | `timeout (number, optional): Timeout in milliseconds for requests to this MCP server.` — code: `export const MCP_DEFAULT_TIMEOUT_MSEC = 10 * 60 * 1000; // default to 10 minutes` and `await mcpClient.connect(transport, { timeout: mcpServerConfig.timeout ?? MCP_DEFAULT_TIMEOUT_MSEC })` and `await this.client.callTool({...}, undefined, { timeout: this.timeout })` where `this.timeout` is set from `mcpServerConfig.timeout ?? MCP_DEFAULT_TIMEOUT_MSEC` | v0.61.0 |
| 9 | `MCPServerConfig.timeout` field declaration | `src/install/vendor_gemini.rs:195-199` (native for every transport) | `timeout` native, milliseconds, optional number | unchanged | https://github.com/google-gemini/gemini-cli/blob/v0.61.0/packages/core/src/config/config.ts#L502 | `// Common\n    readonly timeout?: number,` (constructor param of `export class MCPServerConfig`) | v0.61.0 |

Unsourced:
- (none — every claim above carries a primary quote)

Feed notes:
- Newest tag or heading seen: `v0.61.0` (stable, published 2026-09-23T23:59:15Z). Nightlies/previews up to `v0.63.0-nightly.20260926...` exist but are skipped per Feed instructions (stable v* only).
- Cursor found: n/a — cursor was `—` (never checked before this sweep). Record `v0.61.0` / 2026-09-23 as the new cursor.
- Vendor version current today (2026-09-27): `v0.61.0` (stable) per https://github.com/google-gemini/gemini-cli/releases.
- Anything grim renders or documents that the feed shows changed but no ledger row covers: none found. Changelog entries in the 2026-07-17→2026-09-27 window (`v0.52.0` 2026-07-22, `v0.53.0` 2026-07-28, `v0.54.0` 2026-08-06, `v0.58.0` 2026-09-01, `v0.59.0` 2026-09-08 — https://github.com/google-gemini/gemini-cli/blob/v0.61.0/docs/changelogs/index.md) are security/triage/CI focused (MCP OAuth SSRF mitigation, fail-closed workspace trust filtering of `mcpServers` in v0.59.0; path/symlink handling in v0.58.0) and do not change any skill/rule/agent/MCP/hook/frontmatter/config-path/env-var shape that grim renders or documents.

## Verifier sign-off

One opus verifier (C-008) re-fetched each claim that drives a docs, comment,
watchlist-wording or issue-draft change. No claim in this pass drives a
renderer, metadata or validation change unless Routing says so.

| Claim | Verdict | Evidence URL | Verbatim quote / note |
|---|---|---|---|
| V1 gemini timeout | CONFIRMED | https://github.com/google-gemini/gemini-cli/blob/v0.61.0/packages/core/src/tools/mcp-client.ts (L96, L1362, L1461, L1916); https://github.com/google-gemini/gemini-cli/blob/v0.61.0/docs/reference/configuration.md (L2523) | `export const MCP_DEFAULT_TIMEOUT_MSEC = 10 * 60 * 1000; // default to 10 minutes`. Same `mcpServerConfig.timeout ?? MCP_DEFAULT_TIMEOUT_MSEC` is passed to `mcpClient.connect(transport, {timeout})` and to `McpCallableTool`, which calls `this.client.callTool(..., { timeout: this.timeout })`. Docs: "`timeout` (number, optional): Timeout in milliseconds for requests to this MCP server." |

## Live CLI check

n/a — `sweep` depth on a Tier 2 harness (the live check is `deep` only).

## Routing

| Claim | Class | Landed as |
|---|---|---|
| 8, 9 / V1 MCP `timeout` | (a) + (c) | `mcp-servers.md` timeout row; new watchlist row; issue draft 1 |
| 1, 2, 3, 4 | (a) | watchlist rows dated |
| 6, 7 `GEMINI_CLI_HOME`, no `GEMINI_CONFIG_DIR` | (a) | new `## Config roots and env vars` row |
| 5 Antigravity sunset row | none | already closed; left as dated |
| v0.59.0 trust filtering of `mcpServers` | finding | project MCP entries load only in a trusted workspace; grim's file is unchanged |

Security: none found.

## Issue drafts

Filed as https://github.com/grimoire-rs/grimoire/issues/146.

### 1. Gemini: MCP `timeout` also bounds every tool call

**Labels:** `vendor:gemini`, `needs-design`

grim's descriptor `timeout` is documented as a startup/tool-fetch timeout in
milliseconds, and grim writes it verbatim into Gemini's `timeout`. In Gemini
CLI v0.61.0 the same value is passed to `mcpClient.connect` and to every
`callTool`, with `MCP_DEFAULT_TIMEOUT_MSEC = 10 * 60 * 1000`
([mcp-client.ts @ v0.61.0](https://github.com/google-gemini/gemini-cli/blob/v0.61.0/packages/core/src/tools/mcp-client.ts)).
A descriptor tuned for startup (say 7000 ms) therefore also aborts any
Gemini tool call longer than 7 s. Options: keep the projection and document
it (done in `mcp-servers.md`), or stop projecting `timeout` for Gemini.
Dropping the key changes the rendered bytes of existing installs, so it
needs the self-heal proof and an upgrade note.

## Friction

- Researcher sandboxes denied network `curl` (and, for some, `gh`); the brief's "`curl -sL <url>` is fine" tip was wrong there. Researchers fell back to WebFetch, whose text is model-extracted, so quotes from vendor sites are summarizer-sourced; GitHub sources were read raw via `gh api` where allowed. Brief fixed.
- Driver cost: the brief told researchers to return only the table, so a 14-target run would have re-typed every table. This run gave each researcher one scratchpad output file and assembled the artifacts from it. Brief now names that file.

## Skill changes

One `chore(skills):` commit for the whole WP-G run, [f9effcc1](https://github.com/grimoire-rs/grimoire/commit/f9effcc1): researcher-brief scratchpad output file, no-curl fallback and re-fetch warning; domains.md docs and feed URLs (kiro, junie, amp, antigravity, goose, openclaw, qoder), `JUNIE_HOME` / `OPENCLAW_HOME` cells, section-default and stamp re-dating note; SKILL.md one artifact per target in a `--domain` run.
