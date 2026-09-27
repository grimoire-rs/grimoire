# Research: upstream refresh — copilot (deep)

## Metadata
**Date:** 2026-09-27 · **Domain:** vendors · **Depth:** `copilot deep named harness`
**Triggered by:** /upstream-refresh copilot (WP-F, plan_harness_capability_freshness)
**Expires:** 2027-03-27

Vendor version at check: GitHub Copilot CLI **1.0.88** (GH release
`v1.0.88`, 2026-09-22; npm `@github/copilot` 1.0.88). Prereleases
`v1.0.89-0` … `v1.0.89-5` were skipped per the `-N` filter. First pass on
this row, so the cursor was `—`; the in-repo `changelog.md` was swept from
2026-07-17 (the newest `verified` date among the Copilot rows) to today,
and the package's own `changelog.json` (shipped inside the 1.0.88 npm
tarball) was read whole for older history. Feed cursor set to `v1.0.88`.

## Claims

Researcher (sonnet) table. Numbered rows re-verify ledger claims; `I` rows
are deep-inventory differences. `L` rows are the pass driver's own live
findings (sandbox, `env -i`, no login). Quotes are verbatim. `ref` =
<https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-command-reference>.

| # | Claim | Ledger location | Old value | New value | Source URL | Verbatim quote | Vendor version |
|---|---|---|---|---|---|---|---|
| 1 | Global MCP env substitution | watchlist; `vendor_copilot.rs` global `mcp_entry`; `mcp-servers.md`; `clients.md#gap-copilot-env` | skip + warn; shipped 0.0.406, regressed 0.0.407 | **shipped**: `env` and `headers` expand `$VAR`, `${VAR}`, `${VAR:-default}`; [#1403](https://github.com/github/copilot-cli/issues/1403) closed 2026-04-08 as fixed | ref#mcp-server-configuration | "`env` \| No \| Environment variables. Supports `$VAR`, `${VAR}`, and `${VAR:-default}` expansion." | 1.0.88 |
| 2 | `COPILOT_HOME` override, VS Code caveat | watchlist; `subsystem-file-structure.md`; `configuration.md` | VS Code's embedded CLI ignores it ([microsoft/vscode#314806](https://github.com/microsoft/vscode/issues/314806), open) | #314806 **closed**, fixed by [microsoft/vscode#314917](https://github.com/microsoft/vscode/pull/314917); narrower [microsoft/vscode#331073](https://github.com/microsoft/vscode/issues/331073) (Skills dialog, portable mode) open | [cli-config-dir-reference](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference) | "`COPILOT_HOME` replaces the entire `~/.copilot` path." | docs current; VS Code PR merged 2026-07-31 |
| 3 | Vendor skill frontmatter | watchlist | empty registry | unchanged: open Agent Skills standard, no Copilot-only keys | [about-agent-skills](https://docs.github.com/en/copilot/concepts/agents/about-agent-skills) | "The Agent Skills specification is an open standard, used by a range of different AI systems." | docs current |
| 4 | `.agent.md` extension | watchlist | grim emits `<name>.md`; spec requires `.agent.md` | docs unchanged (see L3 for the live result) | [create-custom-agents-for-cli](https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/create-custom-agents-for-cli) | "Each custom agent is defined by a Markdown file with an `.agent.md` extension." | docs current |
| 5 | `excludeAgent` values | watchlist; `COPILOT_RULE_FIELDS` | two literals; third proposed | unchanged; [discussion #195217](https://github.com/orgs/community/discussions/195217) still open | [custom-instructions-path.md](https://github.com/github/docs/blob/main/data/reusables/copilot/custom-instructions-path.md) | "Use either `\"code-review\"` or `\"cloud-agent\"`." | docs current |
| 6 | `ws` MCP transport | watchlist | not documented | unchanged: `local`/`stdio`, `http`/`streamable-http`, `sse` | ref#transport-types | "`sse` \| Remote server using Server-Sent Events transport." | docs current |
| 7 | MCP OAuth | watchlist | no native surface | **shipped**: `oauthClientId`, `oauthPublicClient`, `oauthGrantType`, `oidc` on remote entries | ref#remote-server-configuration-fields | "`oauthGrantType` \| No \| OAuth grant type: `\"authorization_code\"` (default, browser-based flow) or `\"client_credentials\"` (fully headless, no browser or callback)." | `client_credentials` since 1.0.40 |
| 8 | Rule and agent keys | `vendor_copilot.rs` module doc; `vendor-metadata.md` | rules `applyTo` + `excludeAgent`; agents `name`, `description`, `model`, `tools` | rule keys unchanged; agent key set is larger (I1) | [custom-agents-configuration](https://docs.github.com/en/copilot/reference/custom-agents-configuration) | "`infer` \| boolean \| **Retired**. Use `disable-model-invocation` and `user-invocable` instead." | docs current |
| 9 | Global instructions dir | `vendor_copilot.rs` `rule_path` | `~/.copilot/instructions/<name>.instructions.md` | unchanged; since 0.0.412 | [changelog.md](https://github.com/github/copilot-cli/blob/main/changelog.md) | "Support ~/.copilot/instructions/*.instructions.md files for user-level instructions across all repositories." | 0.0.412 |
| 10 | MCP schema and project file | `mcp-servers.md`; `vendor_copilot.rs` `mcp_config_path` | `.vscode/mcp.json` for "Copilot Chat"; `tools` shown | `tools` **required**; `stdio` aliases `local`; `cwd`, `timeout` documented; the CLI reads `.mcp.json` and `.github/mcp.json`, **not** `.vscode/mcp.json` | [add-mcp-servers](https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/add-mcp-servers#adding-per-repository-mcp-servers) | "The `.vscode/mcp.json` file for VS Code is not read by Copilot CLI." | docs current |
| 11 | Skill dirs | `vendor-metadata.md` | project `.github`/`.claude`/`.agents` skills; global `~/.copilot/skills` | unchanged; global also scans `~/.agents/skills` (but see L5) | about-agent-skills | "Personal skills, stored in your home directory and shared across projects (`~/.copilot/skills` or `~/.agents/skills`)." | docs current |
| 12 | `XDG_CONFIG_HOME` interplay | `vendor_copilot.rs` `global_native_root` doc | undocumented, [github/copilot-cli#1750](https://github.com/github/copilot-cli/issues/1750) | **resolved**: XDG no longer read since 1.0.1; `COPILOT_HOME` is the only override | #1750 | "We addressed this starting in v1.0.1. The CLI no longer reads `XDG_CONFIG_HOME` or `XDG_STATE_HOME`" | 1.0.1 |
| I1 | Unprojected agent keys | `COPILOT_AGENT_FIELDS` | `tools`, `model` | documented, not in grim's registry: `target`, `disable-model-invocation`, `user-invocable`, `metadata`, `mcp-servers`; changelog adds `include-custom-instructions` (1.0.86), `reasoning-effort` (1.0.66), `skills` (1.0.22), `model` as a list (1.0.83) | custom-agents-configuration; changelog.md | (row 8) | 1.0.88 |
| I2 | Project MCP target the CLI reads | `vendor_copilot.rs` `mcp_config_path` | `.vscode/mcp.json` only | nothing grim writes at project scope reaches the CLI; `.vscode/mcp.json` source removed in 1.0.22, `.github/mcp.json` added in 1.0.61 | add-mcp-servers; changelog.json | (row 10) | 1.0.88 |
| I3 | MCP `timeout`, `cwd` | `vendor_copilot.rs` "no documented Copilot target — dropped" | dropped | both documented (`cwd` local only) | ref#local-server-configuration-fields | "`cwd` \| No \| Working directory for the server." | docs current |
| I4 | Other MCP fields | — | — | `deferTools`, `disableToolCache`, `slowConnectionThresholdMs`, `oidc`; no grim equivalent | ref | "`deferTools` \| No \| `\"auto\"` (default) or `\"never\"`" | docs current |
| I5 | CLI verbs | `deep-pass.md` copilot row | `skill list`, `instruction list`, `mcp list` | also `mcp get` (`--json`, masks secrets), `skill`/`mcp` `enable`/`disable`; `instruction list` since 1.0.85 | changelog.md | "Add `copilot instruction list`" | 1.0.85 |
| I6 | Malformed skill or agent | — | — | errors are surfaced, not skipped silently | changelog.md | "Surface the real load error for malformed custom agents" | 1.0.71 |
| L1 | `${VAR}` expansion, all fields | live (sandbox) | — | args (`${X}`, `$X`, `${X:-d}`), `env` (embedded `x${X}y`), `command` (`${DIR}/dump.sh`), `url` host and path, `headers` (`Bearer ${X}`, `$X`) all expanded when the MCP server started; a `${VAR}` in the URL port or standing for the whole URL did not connect; an unset `${U}` reached the server literally | live run, `copilot -p` with a dead BYOK provider (servers start before the model call) | — | 1.0.88 |
| L2 | Env vars from the CLI itself | `deep-pass.md` copilot telemetry cell | `COPILOT_OFFLINE` unverified | `copilot help environment` documents `COPILOT_OFFLINE` ("skips all network access: GitHub authentication, telemetry, web tools, GitHub MCP server, and auto-update"), `COPILOT_AUTO_UPDATE=false`, `COPILOT_CUSTOM_INSTRUCTIONS_DIRS`; telemetry is otherwise OTel opt-in | `copilot help environment` | "`COPILOT_HOME`: override the directory where configuration and state files are stored; defaults to `$HOME/.copilot`." | 1.0.88 |
| L3 | Plain `.md` agents load | watchlist `.agent.md` row | spec requires `.agent.md` | `copilot -p x --agent no-such-agent` answered "No such agent: no-such-agent, available: fixture-agent" with grim's `fixture-agent.md` in `$COPILOT_HOME/agents/` and, separately, in project `.github/agents/` | live run | — | 1.0.88 |
| L4 | First-party skill YAML (seed 2) | `.claude/skills/{finalize,meta-maintain-config,next,qa-engineer,security-auditor}/SKILL.md` | rejected by `copilot skill list` | root cause: an unquoted `description:` plain scalar containing `: ` (`Flags: …`, `Modes: …`, `Trigger: …`) is invalid YAML; `grim build` rejects the same five with exit 65, so grim has no validation gap | live run; `grim build` | "failed to parse YAML frontmatter: mapping values are not allowed in this context" | 1.0.88 |
| L5 | Pool skills vs `COPILOT_HOME` | `clients.md` pool readers; `POOL_CAPABLE_VENDORS` | Copilot reads the pool at both scopes | with `COPILOT_HOME` set to any dir other than `~/.copilot`, `~/.agents/skills` is not scanned (listed with it unset, missing with `COPILOT_HOME=$HOME/altcopilot`) | live run; changelog.json | "COPILOT_HOME and --config-dir stop loading skills from ~/.agents/skills" | 1.0.66; live 1.0.88 |

## Verifier sign-off

The opus verifier re-fetched every source and re-ran the live probes
(C-008). V1 drives the renderer change; the rest drive watchlist and docs
edits or issue drafts.

| Claim | Verdict | Evidence URL |
|---|---|---|
| V1 `${VAR}` expansion in global `mcp-config.json` (code-driving) | CONFIRMED, with a `url` caveat. Docs: "`env` … Supports `$VAR`, `${VAR}`, and `${VAR:-default}` expansion." Live: command, args, env, url host and path, headers expand; unset stays literal; a port or whole-URL reference is rejected before expansion ("url: Invalid url") and the entry is dropped while others load. The #1403 closer is not GitHub staff (author association `NONE`) | https://docs.github.com/copilot/reference/copilot-cli-reference/cli-command-reference ; https://github.com/github/copilot-cli/issues/1403 |
| V1 minimum version | No minimum documented. Env `${VAR}` stopped expanding from 0.0.407 until about April 2026. An old CLI passes the literal reference; nothing leaks, because grim writes references only | https://github.com/github/copilot-cli/blob/main/changelog.md |
| V1 grammar | CONFIRMED: grim accepts only `${NAME}` and rejects `:-`, so verbatim is an exact identity mapping. Copilot also expands bare `$NAME`, which grim treats as literal; this already applied to every env-free descriptor written before this change | `src/oci/mcp.rs` |
| V2 plain `.md` agents | PARTIAL: the how-to page names `.agent.md`, the reference page says "Use `.agent.md` or `.md` as the file extension"; live, `.md` loads in both dirs | https://docs.github.com/copilot/how-tos/copilot-cli/customize-copilot/create-custom-agents-for-cli |
| V3 `.vscode/mcp.json` not read by the CLI | CONFIRMED ("The `.vscode/mcp.json` file for VS Code is not read by Copilot CLI."; removed 1.0.22; `.github/mcp.json` since 1.0.61; workspace MCP needs a trusted folder) | https://docs.github.com/copilot/how-tos/copilot-cli/customize-copilot/add-mcp-servers |
| V4 native OAuth fields | CONFIRMED: `oauthClientId`, `oauthPublicClient`, `oauthGrantType`, `oidc`; `auth.redirectPort` only in the changelog (1.0.49, 1.0.52). Targets: `client_id` → `oauthClientId`, `callback_port` → `auth.redirectPort`; `scopes` and `auth_server_metadata_url` have none. The loader ignores unknown keys, so acceptance proves nothing | cli-command-reference; changelog.md |
| V5 VS Code `COPILOT_HOME` | CONFIRMED: [microsoft/vscode#314806](https://github.com/microsoft/vscode/issues/314806) closed 2026-07-31 by [microsoft/vscode#314917](https://github.com/microsoft/vscode/pull/314917), milestone 1.132.0 (released 2026-08-05); [microsoft/vscode#331073](https://github.com/microsoft/vscode/issues/331073) open | the three links |
| V6 pool dropped under `COPILOT_HOME` | CONFIRMED (changelog 1.0.66; live) | changelog.md |
| V7 `timeout` / `cwd` | PARTIAL: Copilot's `timeout` is "for tool discovery and tool calls", and the connection budget is floored at 60000 ms. Mapping grim's startup `timeout` would cap every tool call, so it is **not landed**. A relative `cwd` resolves against the session working directory; not landed either (same open question as Codex) | cli-command-reference |
| V8 `COPILOT_OFFLINE`, `COPILOT_AUTO_UPDATE` | CONFIRMED (`copilot help environment`; offline also on the BYOK docs page); no telemetry-only opt-out exists | https://docs.github.com/copilot/how-tos/copilot-cli/customize-copilot/use-byok-models |

Recommendation taken: land V1 with a URL guard (skip a descriptor whose
raw `url` does not parse) and the four doc caveats. Do not land V7.

**`reviewer:spec` (opus) on the renderer diff: APPROVE WITH FIXES, no
Block.** It ran the unit filter (19/19), the `mcp` filter (124/124) and the
Copilot acceptance tests (3/3), and checked Node's `new URL()` against the
candidate URLs. Fixed once:

- Warn: the acceptance test did not assert the pre-heal `outputs_pending`
  entry the docs promise. It does now.
- Warn: a stale `installer.rs` comment still named Copilot's env-ref skip.
- Warn: the April 2026 docs line overstated the evidence; reworded to
  "before about April 2026, 0.0.406 aside".
- Warn: `upgrading.md` read as if 1.0.88 introduced expansion; reworded.
- Suggest taken: the URL guard now fires only for a URL containing `${`,
  so an env-free descriptor renders byte-identically to before; unit cases
  added on both sides of the port edge; the guard comment no longer
  mentions a whole-URL case it never sees; `mcp-servers.md` notes that
  Copilot does not blank credential variables the way Claude does;
  `mcp-spec.md` warns against a port reference.

## Live CLI check

```text
copilot skill list (native linux-x64 binary, env -i, sandbox HOME, COPILOT_HOME, DO_NOT_TRACK, COPILOT_OFFLINE, COPILOT_AUTO_UPDATE) — pass — grim-usage listed as a project skill (.github/skills) and as a personal skill ($COPILOT_HOME/skills)
copilot instruction list — pass — grim's rule listed from .github/instructions (project) and $COPILOT_HOME/instructions (personal)
copilot mcp list, global — pass — grim's stdio entry (local) and the fixture http entry listed as User servers; the env-ref fixture was skipped by grim at global scope (pre-change binary)
copilot mcp list, project — fail — none of grim's .vscode/mcp.json entries appear; the CLI reads .mcp.json/.github/mcp.json only (issue draft 1)
copilot -p x --agent no-such-agent (dead BYOK provider) — pass — grim's fixture agent listed as available from $COPILOT_HOME/agents and from .github/agents
copilot -p x (dead BYOK provider), hand-written ${VAR} entries — pass — expansion confirmed per L1
copilot skill list, the five first-party skills copied into a project — fail before the fix (5 parse errors), pass after
copilot skill list, COPILOT_HOME=$HOME/altcopilot, pool skill in ~/.agents/skills — fail — pool skill not listed (issue draft 3)
```

No login was needed. The npm loader needs `node`, which `env -i` hides, so
the check ran the native binary the optional dependency ships
(`@github/copilot-linux-x64/copilot`); no install script executed. The
builtin `github-mcp-server` did not appear, because `COPILOT_OFFLINE=true`
disables it. The `-p` probes used `COPILOT_PROVIDER_BASE_URL` pointed at a
closed local port, so no model request left the machine and no credential
existed. Sandbox and registry container removed.

## Routing

| Claims | Class | Landed as |
|---|---|---|
| 1, V1, L1 | (b) | `feat(vendor-copilot)` commit: global MCP writes `${VAR}` verbatim, URL-port guard, self-heal proof |
| 2, 5, V5 | (a) | watchlist and docs re-dated |
| 3, 4, V2, 6, 9, 11, 12 | (a) | watchlist rows, `clients.md`, `configuration.md`, `subsystem-file-structure.md` |
| 7, V4 | (a) + (c) | watchlist row; issue draft 2 |
| 10, I2, V3 | (a) + (c) | `mcp-servers.md` limitation bullet; issue draft 1 |
| L5, V6 | (a) + (c) | `clients.md` pool note, `AGENTS.md` clause; issue draft 3 |
| 8, I1 | (c) | issue draft 4 |
| I3, V7 | none | not landed (timeout caps tool calls, cwd not mapped) |
| L4 | fix | `chore(skills)`: five first-party SKILL.md descriptions quoted, structural test added |
| I4-I6 | none | findings only, recorded above |

Hooks: none in scope. Catalog drift review: `mcp-spec.md` updated for
Copilot's native `${VAR}`. Security: no finding.

## Issue drafts

Filed as https://github.com/grimoire-rs/grimoire/issues/144.

### 1. Copilot: project-scope MCP never reaches Copilot CLI

**Labels:** `enhancement`, `vendor:copilot`, `needs-design`

At project scope grim registers a Copilot MCP server in
`<workspace>/.vscode/mcp.json` (`servers` key, `${env:VAR}`), which VS
Code's Copilot Chat reads. Copilot CLI removed `.vscode/mcp.json` as a
source in 1.0.22 and reads `.mcp.json` (walked to the repo root) and
`.github/mcp.json` (since 1.0.61) instead
([add-mcp-servers](https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/add-mcp-servers#adding-per-repository-mcp-servers):
"The `.vscode/mcp.json` file for VS Code is not read by Copilot CLI.").
Live-checked at 1.0.88: `copilot mcp list` in a grim project shows none of
grim's entries. `.mcp.json` is Claude's grim-managed file, so a second
member there would be two state outputs for one pointer (the reason Qoder
writes its own file). Decide whether Copilot gains a second project output
at `.github/mcp.json` (`mcpServers`, `${VAR}` native, `tools` required).
That adds a new output to every existing Copilot project install, so it
needs the layout-move treatment (upgrade note, `outputs_pending`), not a
silent render change. The docs now state the gap
(`mcp-servers.md#limitations`).

### 2. Copilot: project `[server.oauth]` onto Copilot's native OAuth fields?

**Labels:** `enhancement`, `vendor:copilot`, `needs-design`

grim declines any descriptor with `[server.oauth]` for Copilot. Copilot
CLI now has native OAuth fields on remote entries
([cli-command-reference](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-command-reference)):
`oauthClientId`, `oauthPublicClient`, `oauthGrantType`
(`authorization_code` \| `client_credentials`) and `oidc`, plus
`auth.redirectPort`, which appears only in the changelog (1.0.49; 1.0.52
migrates the legacy `oauth.clientId`/`oauth.callbackPort`). Of grim's
`McpOAuth`, `client_id` → `oauthClientId` and `callback_port` →
`auth.redirectPort` have targets; `scopes` and `auth_server_metadata_url`
do not, and the loader silently ignores unknown keys. Whether `${VAR}`
expands inside `oauthClientId` is unverified. Decide whether to project a
partial block (dropping scopes and the metadata URL with a warning), and
whether `auth.redirectPort` is stable enough while it is undocumented.
Either way a skip becomes an install for descriptors skipped before, so
the upgrade note must say so. Same question as the OpenCode and Codex
drafts; one decision could cover all three.

### 3. Copilot: `shared_skills` writes where Copilot stops reading once `COPILOT_HOME` is set

**Labels:** `bug`, `vendor:copilot`, `needs-design`

`copilot` is in `POOL_CAPABLE_VENDORS`, so
`[options.vendors.copilot].shared_skills = true` moves its global skills to
`$HOME/.agents/skills`. Copilot CLI 1.0.66 stopped scanning
`~/.agents/skills` whenever `COPILOT_HOME` (or `--config-dir`) is set
([changelog](https://github.com/github/copilot-cli/blob/main/changelog.md):
"COPILOT_HOME and --config-dir stop loading skills from ~/.agents/skills").
Live-checked at 1.0.88: a pool skill lists with `COPILOT_HOME` unset or
equal to `~/.copilot`, and is missing with `COPILOT_HOME=$HOME/altcopilot`.
A user with both settings gets global skills Copilot never loads, and
nothing warns. Options: warn at install when the opt-in and a
non-default `COPILOT_HOME` meet; or render Copilot's global skills to the
native root in that case (a layout move for those users). The docs now
state the upstream behavior (`clients.md`, pool readers paragraph).

### 4. Copilot: custom-agent frontmatter keys grim cannot author

**Labels:** `enhancement`, `vendor:copilot`

`COPILOT_AGENT_FIELDS` holds `tools` and `model`. The
[custom-agents reference](https://docs.github.com/en/copilot/reference/custom-agents-configuration)
documents `target` (`vscode` \| `github-copilot`),
`disable-model-invocation`, `user-invocable` and `metadata`, and the CLI
changelog adds `include-custom-instructions` (1.0.86), `reasoning-effort`
(1.0.66), `skills` (1.0.22) and a list-valued `model` (1.0.83). Each
registry row is a permanent contract, so add keys on demand. The scalar
ones (`disable-model-invocation`, `user-invocable`,
`include-custom-instructions` as bools; `target`, `reasoning-effort` as
enums) fit today's `FieldType`; `skills`, `mcp-servers` and a model list
need the structured-metadata work (`adr_structured_vendor_metadata.md`).

## Friction

- **Copilot CLI help is on the binary, not only the docs.** `copilot help
  environment` documents `COPILOT_OFFLINE` and `COPILOT_AUTO_UPDATE`, which
  the deep-pass copilot row called unverified. The npm tarball also ships
  the full release history as `changelog.json` in the package cache, which
  answered the version questions faster than the web. Fixed in
  `deep-pass.md`.
- **The npm loader needs `node`**, hidden by `env -i`; the native binary
  of the optional dependency runs without it. Fixed in `deep-pass.md`.
- **No agent list verb.** `copilot -p x --agent no-such-agent` lists the
  loaded agents before any model call. Added to `deep-pass.md` and
  `domains.md`.
- **List verbs cannot prove `${VAR}` expansion**: `mcp get` shows config as
  written. Starting a session against a closed BYOK port makes the CLI
  spawn MCP servers, so a dump fixture proves it offline. Recorded in
  `deep-pass.md`.
- **The deep-pass copilot note said grim's project MCP entries list** (by
  implication); they never do, because the CLI does not read
  `.vscode/mcp.json`. The note now says so, so the next pass does not
  mistake it for a render bug.
- **Seed findings stayed in `domains.md` after resolution.** Replaced with
  a "none open" line that says how to add one.
- **zsh does not word-split** `R $c` again (same class as the opencode
  note); the loop was rewritten with explicit arguments. No new fix.
- **Researcher overstatements the verifier corrected:** "staff comment" on
  #1403 (the closer is not staff); "docs require `.agent.md`" (the
  reference allows `.md`); `timeout` looked like a free mapping but caps
  every tool call. No brief change: the verifier gate did its job.
- **Verifier-driven code change:** the URL guard was added after the first
  sign-off; its unit case for a whole-URL reference was dropped because
  grim's descriptor validation already rejects that input.
- No researcher retries; three items were unsourced and dropped (the exact
  fix version of #1403, a telemetry-only opt-out, a `--offline` flag).

## Skill changes

A `chore(skills)` commit follows this record. It updates the
`deep-pass.md` copilot row (native binary, `COPILOT_OFFLINE` and
`COPILOT_AUTO_UPDATE` opt-outs, `mcp get` and the `--agent` probe) and the
copilot note (node-less loader, project MCP never lists, the dead-BYOK-port
recipe for proving expansion), the `domains.md` copilot live-check cell,
and replaces the resolved seed findings with a "none open" line.
