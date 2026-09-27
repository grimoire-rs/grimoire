# Research: upstream refresh — kilo (sweep)

## Metadata
**Date:** 2026-09-27 · **Domain:** vendors · **Depth:** `kilo sweep never checked`
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

Vendor version at check: Kilo **v7.8.1** (2026-09-25). Feed: GH
`Kilo-Org/kilocode` releases (`v*`; the `jetbrains/v*` train excluded);
cursor was `—`, **v7.8.1** recorded. Claims checked against source and the
in-repo docs at the `v7.8.1` tag.

## Claims

Researcher (sonnet) table, verbatim. Numbered rows re-verify ledger claims;
`I` rows are changelog or surface differences found in the sweep.

| # | Claim | Ledger location | Old value | New value | Source URL | Verbatim quote | Vendor version |
|---|---|---|---|---|---|---|---|
| 1 | Global pool support | `.claude/rules/vendor-capability-watchlist.md` "Global pool support \| Kilo" row | NOT pool-capable (partial member: project `.agents/skills` loads by default, no global support) | **Contradicted.** Kilo's CLI loads `~/.agents/skills/` globally by default, no config entry or plugin needed — the documented "Action when shipped" condition is met. | https://github.com/Kilo-Org/kilocode/blob/v7.8.1/packages/kilo-docs/pages/customize/skills.md | "To share personal skills across projects, install them at `~/.agents/skills/<name>/SKILL.md`. Kilo discovers this user-level directory by default, without a `skills.paths` entry or a plugin to register the skills." | v7.8.1 (current, 2026-09-25) |
| 2 | "Not pool-capable — partial member" / watchlisted on #10569 | `src/install/vendor_kilo.rs` `//!` module doc | Nearest thing upstream is an open, unmerged feature request; pool membership needs a scope-aware predicate; watchlisted on the upstream issue | **Contradicted by shipped code**, and unrelated to #10569 (that issue tracks `AGENTS.md` fallback, not skills — see row 4). Kilo's own skill scanner globally scans `~/.agents/skills/**/SKILL.md` (and `~/.claude/skills/`) unless `KILO_DISABLE_EXTERNAL_SKILLS` is set (default off). | https://github.com/Kilo-Org/kilocode/blob/v7.8.1/packages/opencode/src/skill/index.ts | "if (!disableExternalSkills) { ... externalDirs.push(AGENTS_EXTERNAL_DIR) ... const root = path.join(global.home, dir) ... scan(state, root, EXTERNAL_SKILL_PATTERN, { dot: true, scope: \"global\", trusted: true, projectRoot })" | v7.8.1 |
| 3 | Kilo global skills row omits the `.agents/skills` compatibility dir | `docs/src/content/docs/vendor-metadata.md` line 417 (`[Kilo] \| ~/.kilo/skills/<name>/ \| None found in current docs`) | Lists only `~/.kilo/skills/<name>/` for Kilo's global scope, unlike the shared-pool vendors listed as `$HOME/.agents/skills/<name>/ (shared pool)` | Page is stale by the same fact as row 1: Kilo's CLI also auto-loads `~/.agents/skills/` and `~/.claude/skills/` globally as "Compatibility Directories." | https://github.com/Kilo-Org/kilocode/blob/v7.8.1/packages/kilo-docs/pages/customize/skills.md | "Compatibility Directories ... \| For interoperability with other tools, the CLI also loads skills from: - `~/.claude/skills/` and `.claude/skills/` - Claude Code compatibility - `~/.agents/skills/` and `.agents/skills/` - Open agent standard" | v7.8.1 |
| 4 | Kilo-Org/kilocode#10569 state | Sweep target named in brief | "open, unmerged upstream feature request" (per watchlist row 1's old text) | Not open. Auto-closed `state_reason: not_planned` on 2026-07-27 for 60 days' inactivity — never merged, never implemented. Also scoped to `~/.agents/AGENTS.md` instruction fallback, not skills. | https://github.com/Kilo-Org/kilocode/issues/10569 | "To stay organized issues are automatically closed after 60 days of no activity. If the issue is still relevant please reopen it or create a fresh new one." | n/a (issue tracker, closed 2026-07-27) |
| 5 | `~/.kilocode` vs `~/.kilo` write root / order | `.claude/rules/vendor-capability-watchlist.md` "`~/.config/kilo/` vs `~/.kilo/`" row; `src/install/vendor_kilo.rs` `globalDirs()` claim | write root is `~/.kilo`; `globalDirs()` returns `[~/.kilocode, ~/.kilo]` | unchanged | https://github.com/Kilo-Org/kilocode/blob/v7.8.1/packages/opencode/src/kilocode/paths.ts | "/** Global Kilo directories in user home: ~/.kilocode and ~/.kilo (legacy first, .kilo wins later) */ export function globalDirs(): string[] { return [path.join(home(), \".kilocode\"), path.join(home(), \".kilo\")] }" | v7.8.1 |
| 6 | `.kilocode` legacy dir never written / deprecated | `.claude/rules/vendor-capability-watchlist.md` "`.kilocode` legacy dir" row | never written; accepted for detection only; deprecated upstream | unchanged (still legacy/read-only fallback) — the specific "EOL 2026-07-31" date could not be reconfirmed against a primary source this pass; see Unsourced | https://github.com/Kilo-Org/kilocode/blob/v7.8.1/packages/opencode/src/kilocode/docs/migration.md | "Kilo scans canonical `.kilo/skill/` and `.kilo/skills/` directories alongside legacy `.kilocode/` equivalents. ... Canonical `.kilo/` skills take precedence over legacy `.kilocode/` skills at the same project level." | v7.8.1 |
| 7 | MCP env-ref form is `{env:VAR}`, not `${VAR}` | `.claude/rules/vendor-capability-watchlist.md` "MCP env-ref form" row; `src/install/vendor_kilo.rs` module doc | substitution form is `{env:VAR}` | unchanged, now also confirmed in vendor docs (previously only source-confirmed) | https://github.com/Kilo-Org/kilocode/blob/v7.8.1/packages/kilo-docs/pages/automate/mcp/using-in-cli.md | "Use `{env:VARIABLE_NAME}` syntax in config files to reference environment variables ... \"headers\": { \"Authorization\": \"Bearer {env:MY_API_KEY}\" }" | v7.8.1 |
| 8 | Rule kind declined for Kilo (no per-file scoping key) | `.claude/rules/vendor-capability-watchlist.md` "Rule + Agent kinds \| Goose, Warp, Droid, OpenClaw, Kilo" row (Kilo part, Rule half) | declined; monolithic instruction files with no in-file scoping key | unchanged — rules are a flat `instructions` array of file paths/globs in `kilo.jsonc`, no per-file frontmatter scoping key shipped | https://github.com/Kilo-Org/kilocode/blob/v7.8.1/packages/kilo-docs/pages/customize/custom-rules.md | "Project rules are configured via the `instructions` key in your project's `kilo.jsonc` file. ... Each entry points to a file path or glob pattern." | v7.8.1 |
| 9 | Agent kind declined for Kilo (no installable subagent file format) | `.claude/rules/vendor-capability-watchlist.md` "Rule + Agent kinds" row (Kilo part, Agent half); `src/install/vendor_kilo.rs` module doc ("Custom \"modes\" are not an installable subagent file format") | declined; custom "modes" are not an installable subagent file format | **Contradicted.** Kilo now documents an installable markdown subagent format with YAML frontmatter (`description`, `mode`, `model`, `temperature`, `permission`) at `~/.config/kilo/agents/` (global) and `.kilo/agents/` (project); filename minus `.md` is the agent name. The "Action when shipped" condition is met. | https://github.com/Kilo-Org/kilocode/blob/v7.8.1/packages/kilo-docs/pages/customize/custom-subagents.md | "Define agents as markdown files with YAML frontmatter. Place them in: - **Global**: `~/.config/kilo/agents/` - **Project-specific**: `.kilo/agents/` The **filename** (without `.md`) becomes the agent name." | v7.8.1 |
| 10 | opencode lineage — Kilo's codebase is built on opencode | `.claude/rules/vendor-capability-watchlist.md` "opencode lineage" row; `src/install/vendor_kilo.rs` module doc | Kilo's current codebase is built on opencode, which grim supports independently | unchanged | https://github.com/Kilo-Org/kilocode/releases/tag/v7.7.9 | "Adopt OpenCode v1.18.19 through v1.18.20 improvements, including subagent error and permission handling in non-interactive runs, network and stream error retry coverage, Cerebras completion limit handling, Cloudflare AI Gateway support, and TUI reasoning status updates." | v7.7.9 (2026-09-23) |
| I1 | `${env:...}`/`${file:...}` placeholders in untrusted project skills/agents/commands/instructions now load instead of erroring | Sweep finding — no existing ledger row | n/a (not previously documented) | New behavior change (PR #14345, shipped v7.7.6): a `$`-prefixed placeholder like `${env:VAR}` in untrusted project markdown (skills, agents, commands, instructions) is now kept literal instead of raising a false `ConfigInvalidError`; untrusted JSON config still rejects it. Does not change the MCP env-ref token itself (`{env:VAR}`, no `$`), which the replacer still matches literally per row 7. | https://github.com/Kilo-Org/kilocode/pull/14345 | "The config variable replacer matches `{env:...}` and `{file:...}` tokens anywhere in the text... The replacer now treats a `$`-prefixed brace as literal in untrusted markdown and prompt text, where these placeholders are documentation." | v7.7.6 (2026-09-21) |

Unsourced:
- Exact `.kilocode` EOL date (2026-07-31) — searched `specs/v2/config.md`, `packages/kilo-docs/pages/customize/agents-md.md`, and the opencode CHANGELOG for an explicit EOL date; found only "legacy"/"deprecated" language (see row 6), no vendor-stated end-of-life date to confirm or refute 2026-07-31.
- `kilocode.ai` → `kilo.ai` 308 redirect (client-name rebrand claim in `vendor_kilo.rs`) — a direct `curl -I https://kilocode.ai` was denied by the sandbox (network egress to that host not permitted this session); not re-verified.

Feed notes:
- Newest tag or heading seen: `v7.8.1` (2026-09-25T17:16:45Z), plus JetBrains-track `jetbrains/v7.1.8` (2026-09-25T20:18:14Z, excluded — separate release train). Cursor found: n/a — cursor was `—`; recommended new cursor is `v7.8.1`.
- Vendor version current today: `v7.8.1`, published 2026-09-25, via `gh api repos/Kilo-Org/kilocode/releases`.
- Anything grim renders or documents that the feed shows changed but no ledger row covers: the installable markdown subagent format (row 9) and the default global `.agents/skills` scan (rows 1-3) are the two structural changes; PR #14345 (row I1) is a smaller behavioral fix worth knowing if grim ever authors example `${env:...}` text into a Kilo skill/agent file.

## Verifier sign-off

One opus verifier (C-008) re-fetched each claim that drives a docs, comment,
watchlist-wording or issue-draft change. No claim in this pass drives a
renderer, metadata or validation change unless Routing says so.

| Claim | Verdict | Evidence URL | Verbatim quote / note |
|---|---|---|---|
| V26 kilo .agents/skills + env | CONFIRMED | Kilo-Org/kilocode@v7.8.1 packages/kilo-docs/pages/customize/skills.md l.50/87 ; packages/opencode/src/skill/index.ts l.224-253 ; packages/opencode/src/effect/runtime-flags.ts l.4/22 | "Kilo discovers this user-level directory by default, without a `skills.paths` entry or a plugin to register the skills." The env var is `KILO_DISABLE_EXTERNAL_SKILLS` (`bool(...)` = `Config.withDefault(false)`), so it defaults to false and scanning is on. Note: the same `if (!disableExternalSkills)` block also gates the project `.agents/skills` walk-up, not only the global scan. |
| V27 kilo #10569 | CONFIRMED | https://github.com/Kilo-Org/kilocode/issues/10569 | The issue is titled "Feat: Support global ~/.agents/AGENTS.md fallback path". It is closed as not_planned, closed_at 2026-07-27T07:02:32Z by github-actions[bot]. Bot comment: "To stay organized issues are automatically closed after 60 days of no activity." |
| V28 kilo markdown subagents | CONFIRMED | Kilo-Org/kilocode@v7.8.1 packages/kilo-docs/pages/customize/custom-subagents.md l.91-94 | "Define agents as markdown files with YAML frontmatter. Place them in:" / "**Global**: `~/.config/kilo/agents/`" / "**Project-specific**: `.kilo/agents/`" |

**reviewer:spec (opus) on the pool diff:** APPROVE — Block none, Warn none.
It checked that the scope is exactly the two roster names, that the native
default path and record anchors are unchanged, that nothing writes the
singular `.agent/skills` or `.kilocode`, and that the acceptance tests are
non-vacuous. Three suggestions were taken: the test docstring now says the
re-install proves idempotence of the default render, standing in for upgrade
self-heal because no default path or renderer changed; the `vendor-metadata.md`
rows say "native by default"; and a `clients.md` line was re-wrapped. The first
reviewer spawn went silent and was retried once.

## Live CLI check

n/a — `sweep` depth on a Tier 2 harness (the live check is `deep` only).

## Routing

| Claim | Class | Landed as |
|---|---|---|
| 1, 2, 3 / V26 global `~/.agents/skills` read by default | (b) | `kilo` joins `POOL_CAPABLE_VENDORS` — [845ea76f](https://github.com/grimoire-rs/grimoire/commit/845ea76f) |
| 4 / V27 #10569 | (a) | the row no longer cites it (it was about `AGENTS.md` fallback, closed not_planned) |
| 9 / V28 markdown subagents | (a) + (c) | multi-vendor Rule + Agent row; module doc; issue draft 1 |
| 5, 6, 7, 10 | (a) | rows dated; stamp dated |
| I1 `${env:…}` literal in untrusted markdown (v7.7.6) | finding | grim writes no such placeholder |

Security: none found.

## Issue drafts

Filed as https://github.com/grimoire-rs/grimoire/issues/149.

### 1. Kilo: enable the Agent kind — markdown subagents shipped

**Labels:** `enhancement`, `vendor:kilo`, `needs-design`

Kilo documents "markdown files with YAML frontmatter" at
`~/.config/kilo/agents/` (global) and `.kilo/agents/` (project), the filename
being the agent name; frontmatter `description`, `mode`, `model`,
`temperature`, `permission`
([custom-subagents.md @ v7.8.1](https://github.com/Kilo-Org/kilocode/blob/v7.8.1/packages/kilo-docs/pages/customize/custom-subagents.md),
verified 2026-09-27). The watchlist flip condition is met. Note the global
dir is under `~/.config/kilo/`, not grim's `~/.kilo` write root, and the
frontmatter is OpenCode-shaped (`permission` map, `mode`), so the OpenCode
agent renderer is the natural template. Kind enablement: grid, ADR mapping
table, docs matrix, upgrade note.

## Friction

- Researcher sandboxes denied network `curl` (and, for some, `gh`); the brief's "`curl -sL <url>` is fine" tip was wrong there. Researchers fell back to WebFetch, whose text is model-extracted, so quotes from vendor sites are summarizer-sourced; GitHub sources were read raw via `gh api` where allowed. Brief fixed.
- Driver cost: the brief told researchers to return only the table, so a 14-target run would have re-typed every table. This run gave each researcher one scratchpad output file and assembled the artifacts from it. Brief now names that file.

## Skill changes

One `chore(skills):` commit for the whole WP-G run, [f9effcc1](https://github.com/grimoire-rs/grimoire/commit/f9effcc1): researcher-brief scratchpad output file, no-curl fallback and re-fetch warning; domains.md docs and feed URLs (kiro, junie, amp, antigravity, goose, openclaw, qoder), `JUNIE_HOME` / `OPENCLAW_HOME` cells, section-default and stamp re-dating note; SKILL.md one artifact per target in a `--domain` run.
