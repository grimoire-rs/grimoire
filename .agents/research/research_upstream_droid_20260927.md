# Research: upstream refresh — droid (sweep)

## Metadata
**Date:** 2026-09-27 · **Domain:** vendors · **Depth:** `droid sweep never checked`
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

Vendor version at check: Factory CLI **v0.228.0** / Desktop v0.185.0
(2026-09-26). Feed: page <https://docs.factory.ai/changelog/release-notes>;
cursor was `—`, **CLI v0.228.0** recorded. No changelog entry 2026-07-17 →
2026-09-27 announces the surfaces below; they come from re-reading the
configuration docs against the ledger (hook risk levels and org skill
policy are the only adjacent entries).

## Claims

Researcher (sonnet) table, verbatim. Numbered rows re-verify ledger claims;
`I` rows are changelog or surface differences found in the sweep.

| # | Claim | Ledger location | Old value | New value | Source URL | Verbatim quote | Vendor version |
|---|---|---|---|---|---|---|---|
| 1 | Rule kind decline condition (per-file scoping key) | `.claude/rules/vendor-capability-watchlist.md:183` | Declined — "monolithic instruction files with no in-file scoping key ... Droid `AGENTS.md`-style hierarchical-by-location" | unchanged — AGENTS.md discovery is still hierarchical by file location only; no `paths:`-style scoping key documented | https://docs.factory.ai/cli/configuration/agents-md | "Nested project files refine root project files for a specific directory tree. Project files should override personal defaults." | not stated on page (site-wide current: CLI v0.228.0 / Desktop v0.185.0, 2026-09-26) |
| 2 | Agent kind decline condition (installable agent file format) | `.claude/rules/vendor-capability-watchlist.md:183` | Declined — "agents runtime- or UI-only everywhere"; action: "enable per vendor when ... an installable agent file format ships" | **condition now met for Droid** — installable custom-agent ("droid") `.md` files with YAML frontmatter now documented at `<repo>/.factory/droids/` (project) and `~/.factory/droids/` (personal); fields `name`, `description`, `model`, `reasoningEffort`, `tools`, `mcpServers`; on name conflict "project definition wins" | https://docs.factory.ai/cli/configuration/custom-droids | "Markdown with YAML frontmatter followed by the system prompt body." | not stated on page (site-wide current: CLI v0.228.0 / Desktop v0.185.0, 2026-09-26) |
| 3 | Skills-pool membership — "`.agents/skills` appears in neither list. Not a shared-pool client." | `src/install/vendor_droid.rs` module doc (`//!` block, skills bullet) | `.agents/skills` (plural, cross-vendor pool) absent from Droid's documented skill locations; only the singular `.agent/skills/` compat dir was known | **changed** — Factory now documents `.agents/skills/**/SKILL.md` (plural) as a compatibility directory at *both* project and personal scope, alongside `.agent/skills/**/SKILL.md` (singular) | https://docs.factory.ai/cli/configuration/skills | "Compatibility: `<repo>/.agents/skills/**/SKILL.md`, `<repo>/.agent/skills/**/SKILL.md`" / "Personal compatibility: `~/.agents/skills/**/SKILL.md`, `~/.agent/skills/**/SKILL.md`" | not stated on page (site-wide current: CLI v0.228.0 / Desktop v0.185.0, 2026-09-26) |
| 4 | MCP decline — "No grim-writable config file surface this wave." | `src/install/vendor_droid.rs` module doc (`//!` block, MCP bullet) | MCP declined; no config file surface documented | **changed** — a structured, grim-writable JSON surface is now documented: `~/.factory/mcp.json` (user), `.factory/mcp.json` in an ancestor dir (folder), `.factory/mcp.json` in project root (project); `mcpServers` object with `type: stdio\|http\|sse`, `disabled`, `disabledTools`, `timeout`, `connectTimeout`, transport-specific `command`/`args`/`env` or `url`/`headers`/`oauth`; `${NAME}` env expansion only (no default-value syntax) | https://docs.factory.ai/cli/configuration/mcp | "Droid expands ${NAME} references in mcp.json against your current shell environment when it connects to a server." | not stated on page (site-wide current: CLI v0.228.0 / Desktop v0.185.0, 2026-09-26) |
| 5 | Rules declined prose — "document no per-file rules surface that can express a rule's `paths`" | `docs/src/content/docs/clients.md:369-372` | Declined, no per-file scoping | unchanged — confirmed again via current AGENTS.md discovery doc (see claim 1); still no `paths`-equivalent key | https://docs.factory.ai/cli/configuration/agents-md | "The current user request takes priority over standing instructions." | not stated on page (site-wide current: CLI v0.228.0 / Desktop v0.185.0, 2026-09-26) |
| 6 | Skills directory paths (project/global) | `docs/src/content/docs/vendor-metadata.md:385,413` | Project `.factory/skills/<name>/`; Global `~/.factory/skills/<name>/`; "None — no `FACTORY_HOME` or `DROID_HOME` appears in current docs" | unchanged for the primary paths themselves, but the page's description is now incomplete — it omits the `.agents/skills` and `.agent/skills` compatibility directories Factory now documents at both scopes (see claim 3) | https://docs.factory.ai/cli/configuration/skills | "Project scope: `<repo>/.factory/skills/<skill-name>/SKILL.md`" / "Personal scope: `~/.factory/skills/<skill-name>/SKILL.md`" | not stated on page (site-wide current: CLI v0.228.0 / Desktop v0.185.0, 2026-09-26) |

Unsourced:
- `FACTORY_HOME` / `DROID_HOME` non-existence — checked https://docs.factory.ai/cli/configuration/skills, /mcp, and /agents-md; none mentions either variable. Absence cannot be backed by a verbatim quote (nothing to quote), so left out of the table; status is consistent with the existing "not found on pages checked" framing in `vendor_droid.rs`.
- Exact ship date of the `.factory/droids/` custom-agent format and the `.agents/skills` / `.agent/skills` compatibility directories — searched the changelog (2026-07-17 to 2026-09-27) for "droids", "custom agent", ".agents/skills", "mcp.json", "compatibility directory": no matching entry. Only indirect corroboration found: several September 19-25 entries about subagent UI (start/stop indicators, delegation) confirm subagents are an actively developed, currently-live feature, but none pins when the file-based format or the skills compat dirs shipped.

Feed notes:
- Newest tag or heading seen: "September 26, 2026 – CLI v0.228.0 / Desktop v0.185.0" (page https://docs.factory.ai/changelog/release-notes). Cursor found: n/a — cursor was `—`.
- Vendor version current today: CLI v0.228.0 / Desktop v0.185.0, per https://docs.factory.ai/changelog/release-notes (2026-09-26 entry).
- Anything grim renders or documents that the feed shows changed but no ledger row covers: none directly from the changelog sweep itself (no entry in the July 17 - September 27 window describes a skills/rules/agent/MCP/hooks/frontmatter/config-path/env-var change) — the substantive gaps found (rows 2, 3, 4 above) came from re-reading Factory's current configuration docs against the existing ledger claims, not from a dated changelog entry. Two changelog items are adjacent but out of grim's current scope: "Hook risk level setting" (2026-09-19, CLI v0.223.0) — grim has no `Hook` `ArtifactKind` for any vendor, so not applicable — and "Organization skill policy" (2026-09-18, CLI v0.222.0, admin-side skill disabling) — an org/UI control, not a file-format or path change.

## Verifier sign-off

One opus verifier (C-008) re-fetched each claim that drives a docs, comment,
watchlist-wording or issue-draft change. No claim in this pass drives a
renderer, metadata or validation change unless Routing says so.

| Claim | Verdict | Evidence URL | Verbatim quote / note |
|---|---|---|---|
| V17 | CONFIRMED | https://docs.factory.ai/cli/configuration/custom-droids | "Markdown with YAML frontmatter followed by the system prompt body"; project `.factory/droids/`, personal `~/.factory/droids/`; fields `name` (required), `description`, `model` (default `inherit`), `reasoningEffort`, `tools`, `mcpServers`. |
| V18 | CONFIRMED | https://docs.factory.ai/cli/configuration/mcp | "Droid expands `${NAME}` references in `mcp.json` against your current shell environment when it connects to a server." User `~/.factory/mcp.json`, project `.factory/mcp.json`, key `mcpServers`, transports stdio/http/sse. Extra: a folder-level (ancestor dir) `.factory/mcp.json`; expansion only in `env`, `headers`, `oauth.clientId/clientSecret`. |
| V19 | CONFIRMED | https://docs.factory.ai/cli/configuration/skills | Compatibility dirs "`<repo>/.agents/skills/**/SKILL.md` and `<repo>/.agent/skills/**/SKILL.md`" and "`~/.agents/skills/**/SKILL.md` and `~/.agent/skills/**/SKILL.md`" (singular `.agent` also scanned). |
| V31 droid pool + Factory compat | CONFIRMED; no explicit compat-vs-.factory rank | src/install/vendor.rs:163-165 ; https://docs.factory.ai/cli/configuration/skills | Droid is absent from POOL_CAPABLE_VENDORS. "Compatibility \| `<repo>/.agents/skills/**/SKILL.md`, `<repo>/.agent/skills/**/SKILL.md`" ; "Personal compatibility \| `~/.agents/skills/**/SKILL.md`, `~/.agent/skills/**/SKILL.md`". "When multiple sources provide the same sanitized skill name, Droid keeps one effective version and shows the others as overridden." "Within one source bucket, duplicate names are invalid configuration." The Factory and Qoder text came via WebFetch because curl was denied, so it may not be byte-exact. |

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
| 3, 6 / V19, V31 `.agents/skills` at both scopes | (b) | `droid` joins `POOL_CAPABLE_VENDORS` — [845ea76f](https://github.com/grimoire-rs/grimoire/commit/845ea76f) |
| 2 / V17 custom droids | (a) + (c) | multi-vendor Rule + Agent row; module doc; issue draft 1 |
| 4 / V18 `.factory/mcp.json` | (a) + (c) | new `MCP kind` row (Cline, Droid, Warp); module doc; issue draft 2 |
| 1, 5 rules | (a) | confirmed unchanged; stamp dated |

Security: none found.

## Issue drafts

Filed as https://github.com/grimoire-rs/grimoire/issues/145.

### 1. Droid: enable the Agent kind — custom droids

**Labels:** `enhancement`, `vendor:droid`, `needs-design`

Factory documents custom droids as "Markdown with YAML frontmatter followed
by the system prompt body" at `<repo>/.factory/droids/` and
`~/.factory/droids/`, fields `name` (required), `description`, `model`
(default `inherit`), `reasoningEffort`, `tools`, `mcpServers`
(<https://docs.factory.ai/cli/configuration/custom-droids>, verified
2026-09-27, CLI v0.228.0). Kind enablement: grid, ADR mapping table, docs
matrix, a `droid.*` registry decision for `reasoningEffort`, upgrade note.

### 2. Droid: enable MCP for `.factory/mcp.json`

**Labels:** `enhancement`, `vendor:droid`, `needs-design`

Factory documents `~/.factory/mcp.json` (user), `.factory/mcp.json` in the
project root and in ancestor folders, key `mcpServers`, types `stdio` |
`http` | `sse`, and "Droid expands `${NAME}` references in `mcp.json`
against your current shell environment" — in `env`, `headers` and
`oauth.clientId`/`clientSecret` only
(<https://docs.factory.ai/cli/configuration/mcp>, verified 2026-09-27).
`${NAME}` matches grim's native reference form. Kind enablement: grid, ADR
mapping table, docs matrix, a JSON splice target, upgrade note.

## Friction

- Factory never ranks its `.agents/skills` compatibility dirs against `.factory/skills`; it keeps one effective version per name and marks the rest overridden (V31). Only a hand-placed duplicate can lose that tie, as for Amp.
- Researcher sandboxes denied network `curl` (and, for some, `gh`); the brief's "`curl -sL <url>` is fine" tip was wrong there. Researchers fell back to WebFetch, whose text is model-extracted, so quotes from vendor sites are summarizer-sourced; GitHub sources were read raw via `gh api` where allowed. Brief fixed.
- Driver cost: the brief told researchers to return only the table, so a 14-target run would have re-typed every table. This run gave each researcher one scratchpad output file and assembled the artifacts from it. Brief now names that file.

## Skill changes

One `chore(skills):` commit for the whole WP-G run, [f9effcc1](https://github.com/grimoire-rs/grimoire/commit/f9effcc1): researcher-brief scratchpad output file, no-curl fallback and re-fetch warning; domains.md docs and feed URLs (kiro, junie, amp, antigravity, goose, openclaw, qoder), `JUNIE_HOME` / `OPENCLAW_HOME` cells, section-default and stamp re-dating note; SKILL.md one artifact per target in a `--domain` run.
