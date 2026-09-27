# Research: upstream refresh — qoder (sweep)

## Metadata
**Date:** 2026-09-27 · **Domain:** vendors · **Depth:** `qoder sweep never checked`
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

Vendor version at check: Qoder CLI **1.1.63** (2026-09-24). domains.md named
<https://qoder.com/changelog>, a JS app WebFetch returns only chrome for; the
researcher used the dated CLI release notes
<https://docs.qoder.com/release-notes/qoder-cli> instead (refined in
domains.md). Cursor was `—`, **CLI 1.1.63** recorded. Notes 2026-09-24 back to
2026-07-17 swept: `.agents/skills` compatibility (1.1.6 → 1.1.11, default on),
recursive skill scan (1.1.9), an "MCP gateway" (1.1.50, undocumented).

## Claims

Researcher (sonnet) table, verbatim. Numbered rows re-verify ledger claims;
`I` rows are changelog or surface differences found in the sweep.

| # | Claim | Ledger location | Old value | New value | Source URL | Verbatim quote | Vendor version |
|---|---|---|---|---|---|---|---|
| 1 | `agentsMdExcludes` scope/format/documentation | `.claude/rules/vendor-capability-watchlist.md:204` (Support-dir exclusion row); `src/install/vendor_qoder.rs` module doc | Named once, in a troubleshooting line, no scope/format/settings-file documented; absent from the settings reference | Now in the settings reference table: `agentsMdExcludes` (`string[]`, default `[]`, restart required) — excludes "project-level / local-level memory files" by glob. Still does **not** confirm it covers `rules/**/*.md` support-directory files (vs. only `AGENTS.md`/`AGENTS.local.md`); the row's "Action when shipped" condition (a usable exclude covering `rules/`) is **not clearly met** | https://docs.qoder.com/cli/settings-reference.md | "Exclude project-level / local-level memory files by glob. Top-level key, supports manual configuration only." | Docs page undated; CLI 1.1.63 current |
| 2 | MCP env-ref (`${VAR}`) expansion undocumented | Watchlist "MCP env-ref expansion" row; module doc | No `${VAR}` expansion documented | unchanged — `mcp-reference.md` describes stdio `env` only as "Environment variables passed to the subprocess", with no substitution syntax anywhere on the page | https://docs.qoder.com/cli/mcp-reference | "Environment variables passed to the subprocess" (full `env` field description; no `${VAR}` syntax appears on the page) | Docs page undated |
| 3 | MCP `ws` transport is `tcp{host,port}`, not a URL | Watchlist "MCP `ws` transport" row; module doc | Documented as `tcp{host,port}` object | unchanged | https://docs.qoder.com/cli/mcp-reference | `"tcp": { "host": "...", "port": ... }` (ws/Type (TCP) table) | Docs page undated |
| 4 | MCP `oauth` block field list | Watchlist "MCP `oauth` block" row; module doc | `{enabled, clientId, clientSecret, authorizationUrl, tokenUrl, scopes, callbackPort}`, no `authServerMetadataUrl` | unchanged | https://docs.qoder.com/cli/mcp-reference | "oauth `object` OAuth authorization configuration (fields include `enabled`, `clientId`, `clientSecret`, `authorizationUrl`, `tokenUrl`, `scopes`, `callbackPort`, etc.)." | Docs page undated |
| 5 | Project MCP file: grim writes `.qoder/settings.json`, never `.mcp.json`; both documented upstream | Watchlist "Project MCP file" row; module doc | Both documented; `.mcp.json` is Claude's grim-managed target | unchanged, but note a doc inconsistency: `mcp-servers.md` lists project scope as `${project}/.qoder/settings.local.json` + `${project}/.mcp.json` (no project-level plain `settings.json`), while `mcp-reference.md` separately lists `<project>/.qoder/settings.json` as a project-level path | https://docs.qoder.com/cli/mcp-servers | "~/.qoder/settings.json" (user-level); "${project}/.qoder/settings.local.json" (local); "${project}/.mcp.json" (project-level) | Docs page undated |
| 6 | IDE path sharing with `qodercli` is inferred, not stated | Watchlist "IDE path sharing" row | Documented for `qodercli` only; IDE sharing inferred | unchanged — `cli/overview` frames the whole product as CLI-only and never mentions a desktop IDE or shared config path | https://docs.qoder.com/cli/overview | "command-line AI Coding Assistant" (page scope; no IDE section or `.qoder` sharing statement found) | Docs page undated |
| 7 | `.agents/skills` pool: no doc says Qoder scans the pool | Watchlist "`.agents/skills` pool" row | NOT pool-capable — native `.qoder/skills` only; no doc says Qoder scans the pool | Changed upstream: CLI 1.1.6 (2026-07-27) added settings to control skill loading from `.agents`; CLI 1.1.7 (2026-07-28) added an `.agents/skills` compatibility **toggle** in `/settings`; CLI 1.1.11 (2026-08-01) made it **enabled by default**. This is changelog-only evidence — no dedicated docs page states project vs. global scope or exact mechanics, so "evidenced at both scopes" is still **not fully met**; re-verify via docs or a direct test before flipping `POOL_CAPABLE_VENDORS` | https://docs.qoder.com/release-notes/qoder-cli | "The .agents/skills compatibility source is now enabled by default." | CLI 1.1.11 (2026-08-01) |
| 8 | `QODER_CONFIG_DIR` replaces `~/.qoder` outright, no `.qoder` segment appended | Module doc; AGENTS.md env table; `docs/src/content/docs/vendor-metadata.md:418` | Replaces root outright (`KIRO_HOME`/`CODEX_HOME` shape) | unchanged | https://docs.qoder.com/cli/config-scope | "The location of the User Configuration Directory can be customized via the `QODER_CONFIG_DIR` Environment Variable. The project-level `.qoder` directory is always located at the project root directory." | Docs page undated |
| 9 | Rule `paths:` frontmatter is native (glob or list), equivalent to `trigger: glob` | Module doc; `docs/src/content/docs/vendor-metadata.md:460` | Native; plain rule installs verbatim | unchanged | https://docs.qoder.com/cli/memory | "`paths`: single glob or list of globs (equivalent to `trigger: glob` + `glob`)" | Docs page undated |
| 10 | Skill paths: `.qoder/skills/<name>/` (project), `<root>/skills/<name>/` (global) | Module doc; `docs/src/content/docs/vendor-metadata.md:388,418` | As stated | unchanged | https://docs.qoder.com/cli/Skills | User-level: `"~/.qoder/skills/{skill-name}/SKILL.md"`; Project-level: `".qoder/skills/{skill-name}/SKILL.md"` | Docs page undated |
| I1 | Sweep: skill directory scanning changed | none (no ledger row covers this) | n/a | New: "Skill directories are now scanned recursively" | https://docs.qoder.com/release-notes/qoder-cli | "Skill directories are now scanned recursively" | CLI 1.1.9 (2026-07-30) |

Unsourced:
- Agent frontmatter shape (`tools` as comma-string or list; `model` incl. `inherit`) for `.qoder/agents/<name>.md` — checked `docs.qoder.com/qoder/custom-agents.md` and `docs.qoder.com/cli/builtins-reference.md`; neither states the YAML/frontmatter schema for a custom subagent file. No primary quote found this pass (module doc's claim traces to the 2026-09-24 verification, not re-confirmed now).
- `qoder.*` registries claim (empty per kind; common `model`/`tools` project as-is) — not independently re-fetched this pass; no new primary source checked, so left out of the table rather than marked "unchanged" without a quote.
- Whether `rules/**/*.md` support-directory files specifically are covered by any exclude key — checked `docs.qoder.com/user-guide/rules.md` and `docs.qoder.com/cli/how-memory-works.md`; neither addresses recursive loading or exclusion for `rules/` subdirectories.
- "Qoder MCP gateway" (CLI 1.1.50, 2026-09-12) — checked `docs.qoder.com/cli/mcp-servers.md`; term does not appear, no config-shape documentation found.
- IDE `~/.qoder` / `.qoder` sharing with `qodercli` — no fetched page discusses a desktop IDE at all.

Feed notes:
- Newest tag or heading seen: "September 24, 2026 - CLI 1.1.63" (https://docs.qoder.com/release-notes/qoder-cli). Cursor found: n/a — cursor was `—`.
- The brief's specified feed, https://qoder.com/changelog, is a JS-rendered SPA — WebFetch returned only page chrome ("Content truncated due to length...") on repeated attempts, and `https://qoder.com/changelog.md` 404'd. Substituted `https://docs.qoder.com/release-notes/qoder-cli`, the dated CLI-specific release notes, as the primary changelog source for the sweep.
- Vendor version current today: CLI 1.1.63 (2026-09-24), per https://docs.qoder.com/release-notes/qoder-cli.
- Anything grim renders/documents that the feed shows changed but no ledger row covers: "Skill directories are now scanned recursively" (CLI 1.1.9, 2026-07-30, https://docs.qoder.com/release-notes/qoder-cli) — no ledger row addresses skill-directory recursion. Also: the `.agents/skills` compatibility-source rollout (CLI 1.1.6 → 1.1.7 → 1.1.11, 2026-07-27 to 2026-08-01) is covered by ledger row 7 above but is sourced only from changelog text — a dedicated docs page confirming project vs. global scope was not found.

## Verifier sign-off

One opus verifier (C-008) re-fetched each claim that drives a docs, comment,
watchlist-wording or issue-draft change. No claim in this pass drives a
renderer, metadata or validation change unless Routing says so.

| Claim | Verdict | Evidence URL | Verbatim quote / note |
|---|---|---|---|
| V29 qoder agentsMdExcludes | CONFIRMED; silent on .qoder/rules | https://docs.qoder.com/cli/settings-reference.md | `agentsMdExcludes` \| `string[]` \| `[]` \| Restart Yes \| "Exclude project-level / local-level memory files by glob. Top-level key, supports manual configuration only." The page does not mention `.qoder/rules`. |
| V30 qoder 1.1.11 | CONFIRMED; scope NOT stated | https://docs.qoder.com/release-notes/qoder-cli (also checked /cli/Skills.md, /extensions/skills.md, /qoder/skills.md, /cli/settings.md, /cli/settings-reference.md) | "September 24, 2026 - CLI 1.1.63" ; "The .agents/skills compatibility source is now enabled by default". No page states project vs user scope. /cli/Skills lists only ~/.qoder/skills and .qoder/skills. |

## Live CLI check

n/a — `sweep` depth on a Tier 2 harness (the live check is `deep` only).

## Routing

| Claim | Class | Landed as |
|---|---|---|
| 1 / V29 `agentsMdExcludes` | (a) | row dated: now in the settings reference, still not stated to cover `rules/**`, so the class-1 repair stays blocked |
| 7 / V30 `.agents/skills` default-on | (a) | row dated: changelog-only, no scope stated — pool flip stays gated on both-scope evidence |
| 5 project MCP file | (a) + (c) | row: two upstream pages disagree on whether project `.qoder/settings.json` is read; issue draft 1 |
| 8 `QODER_CONFIG_DIR` | (a) | new `## Config roots and env vars` row |
| 2, 3, 4, 6, 9, 10 | (a) | confirmed unchanged; stamp dated |
| I1 recursive skill scan | finding | grim writes one dir per skill; nothing moves |

Security: none found.

## Issue drafts

Filed as https://github.com/grimoire-rs/grimoire/issues/153.

### 1. Qoder: confirm that project `.qoder/settings.json` MCP entries load

**Labels:** `vendor:qoder`, `bug?`, `needs-repro`

grim writes project MCP servers into `.qoder/settings.json`. Qoder's
`mcp-servers` page lists the project-level MCP files as
`${project}/.qoder/settings.local.json` and `${project}/.mcp.json`
(<https://docs.qoder.com/cli/mcp-servers>), while `mcp-reference` also lists
`<project>/.qoder/settings.json` (<https://docs.qoder.com/cli/mcp-reference>),
verified 2026-09-27. If the CLI does not read it, every project-scope Qoder
MCP install is dead. Reproduce with `qodercli` in a sandbox; if broken, the
fix moves a written path (layout change, Principle 9).

## Friction

- `qoder.com/changelog` is a JS app; WebFetch returned page chrome only. Feed switched to the docs release notes in domains.md.
- Agent frontmatter shape and the `qoder.*` registry claim were not re-fetched (no page states the subagent schema); that row keeps its 2026-09-24 date.
- Researcher sandboxes denied network `curl` (and, for some, `gh`); the brief's "`curl -sL <url>` is fine" tip was wrong there. Researchers fell back to WebFetch, whose text is model-extracted, so quotes from vendor sites are summarizer-sourced; GitHub sources were read raw via `gh api` where allowed. Brief fixed.
- Driver cost: the brief told researchers to return only the table, so a 14-target run would have re-typed every table. This run gave each researcher one scratchpad output file and assembled the artifacts from it. Brief now names that file.

## Skill changes

One `chore(skills):` commit for the whole WP-G run, [f9effcc1](https://github.com/grimoire-rs/grimoire/commit/f9effcc1): researcher-brief scratchpad output file, no-curl fallback and re-fetch warning; domains.md docs and feed URLs (kiro, junie, amp, antigravity, goose, openclaw, qoder), `JUNIE_HOME` / `OPENCLAW_HOME` cells, section-default and stamp re-dating note; SKILL.md one artifact per target in a `--domain` run.
