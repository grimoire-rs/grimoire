# Research: upstream refresh — cursor (sweep)

## Metadata
**Date:** 2026-09-27 · **Domain:** vendors · **Depth:** `cursor sweep never checked`
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

Vendor version at check: none published — no fetched Cursor page states an
editor version. Feed: page <https://cursor.com/changelog>; cursor was `—`,
newest heading seen **Rollouts and Security Review** (2026-09-23), recorded as
the new cursor. Changelog swept 2026-07-17 → 2026-09-27: no entry changes the
`.mdc` rule format, subagent frontmatter, skill format, `mcp.json` shape or
`CURSOR_CONFIG_DIR`.

**C-011 layout decision.** Question: one artifact per target, or one combined
`research_upstream_vendors_20260927.md` for the 14-target run? Research:
SKILL.md › Evidence says "Each target's pass writes
`.agents/research/research_upstream_<target>_<YYYYMMDD>.md`", and S-001 says
"one run artifact is written per target". Decision: 14 artifacts, one per
harness, each carrying the shared ladder block. The skill already prescribes
this; the refinement commit only makes it explicit for `--domain` runs.

## Claims

Researcher (sonnet) table, verbatim. Numbered rows re-verify ledger claims;
`I` rows are changelog or surface differences found in the sweep.

| # | Claim | Ledger location | Old value | New value | Source URL | Verbatim quote | Vendor version |
|---|---|---|---|---|---|---|---|
| 1 | `CURSOR_CONFIG_DIR` relocates Cursor's config tree; grim does not honor it, status "possibly CLI-only, unverified against the IDE" | `.claude/rules/vendor-capability-watchlist.md:124` | not honored (hardcodes `~/.cursor`); "possibly CLI-only, unverified against the IDE" | refined, not reversed: `CURSOR_CONFIG_DIR` is CLI-only AND file-scoped — it only relocates the CLI's own `cli-config.json` (editor/permissions/model preferences), never `.cursor/rules`, `.cursor/agents`, `.cursor/skills`, or `.cursor/mcp.json`. Grim's non-honoring stance is correct regardless of IDE status, since the surfaces grim writes are outside this variable's scope on the CLI too | https://cursor.com/docs/cli/reference/configuration | "The `cli-config.json` file controls CLI preferences and permissions only, not rules, skills, agents, or MCP paths." / Global path "~/.cursor/cli-config.json"; overrides "`CURSOR_CONFIG_DIR`" and "XDG_CONFIG_HOME" (Linux/BSD) which "uses `$XDG_CONFIG_HOME/cursor/cli-config.json`" | not stated |
| 2 | `CURSOR_CONFIG_DIR` is not documented for the IDE (only possibly the CLI) | `.claude/rules/vendor-capability-watchlist.md:124` | unverified against the IDE | unchanged — still undocumented for the IDE; documented only as a CLI `cli-config.json` override | https://cursor.com/docs/cli/reference/configuration | "Environment variable overrides include `CURSOR_CONFIG_DIR` and \"XDG_CONFIG_HOME\" (Linux/BSD)" (page scope is exclusively `cli/reference/configuration`, no IDE section) | not stated |
| 3 | Skills: `.cursor/skills/<name>/` (project), `~/.cursor/skills/` (global) | `src/install/vendor_cursor.rs` `//!` doc | as stated | unchanged for the paths grim writes; docs additionally disclose Cursor also reads `.agents/skills/`, `.claude/skills/`, `.codex/skills/` (and their `~/` equivalents) as legacy/cross-vendor sources not mentioned in grim's comment | https://cursor.com/docs/skills | "Each skill should be a folder containing a `SKILL.md` file" ... "Cursor also loads skills from Claude and Codex directories: `.claude/skills/`, `.codex/skills/`, `~/.claude/skills/`, and `~/.codex/skills/`" | not stated |
| 4 | Rules: `.cursor/rules/<name>.mdc`; `paths` → `globs` (comma-separated string) + computed `alwaysApply` | `src/install/vendor_cursor.rs` `//!` doc; `docs/src/content/docs/vendor-metadata.md:311` | as stated | unchanged | https://cursor.com/docs/context/rules | "Project rules must use the `.mdc` extension. A plain `.md` file in `.cursor/rules` is ignored by the rules system because it has no frontmatter" ... "Separate multiple patterns with commas" (e.g. "`docs/**/*.md, docs/**/*.mdx`") | not stated |
| 5 | Agents: `.cursor/agents/<name>.md` (project), `~/.cursor/agents/` (global); fields `model`/`readonly`/`is_background` | `src/install/vendor_cursor.rs` `//!` doc; `docs/src/content/docs/vendor-metadata.md:311-313` | as stated | unchanged for grim's emitted fields; docs additionally disclose Cursor also reads `.claude/agents/` and `.codex/agents/` (project) and their `~/` equivalents (user), with `.cursor/` winning name conflicts — not mentioned in grim's comment | https://cursor.com/docs/context/subagents | "Project subagents take precedence when names conflict. When multiple locations contain subagents with the same name, `.cursor/` takes precedence over `.claude/` or `.codex/`." Fields table: `model` (string, default `inherit`), `readonly` (bool, default `false`), `is_background` (bool, default `false`) | not stated |
| 6 | MCP: `.cursor/mcp.json` / `~/.cursor/mcp.json`; stdio needs `type: "stdio"`; env refs `${env:NAME}`; structured oauth/auth shape ≠ grim's `McpOAuth` → skip; no websocket transport | `src/install/vendor_cursor.rs` `//!` doc and `mcp_entry()` | as stated | unchanged | https://cursor.com/docs/context/mcp | "Create `.cursor/mcp.json` in your project for project-specific tools." / "Create `~/.cursor/mcp.json` in your home directory for tools available everywhere." / "type \| Yes \| Server connection type \| `\"stdio\"`" / "`${env:NAME}` environment variables" / "Add an `auth` object to remote server entries that use `url`" with `CLIENT_ID`, `CLIENT_SECRET`, `scopes` | not stated |
| 7 | `{#gap-cursor-globs}`: a comma inside a glob (e.g. brace alternation `{a,b}`) splits the pattern because Cursor globs are comma-joined | `docs/src/content/docs/clients.md:156-161` | still true, cites cursor forum #76648 | unchanged — still true. Cursor's own docs still instruct comma-separation for multiple patterns with no escape mechanism, and the forum thread's own follow-up clarifies Cursor globs are "gitignore-style patterns, not unix-style globs," which is consistent with (not a fix for) the comma-split behavior | https://forum.cursor.com/t/76648 ; https://cursor.com/docs/context/rules | "it looks like it takes the `globs:` line and splits on `,`, then treats each as a separate glob." / follow-up: "It turns out cursor rules uses gitignore-style patterns, not unix-style globs. They've updated their documentation to clarify this." | not stated |

Unsourced:
- Current Cursor editor/app version number ("vendor version current today") — tried `cursor.com/docs/cli/changelog` (CLI-only, dated by day not editor version), `cursor.com/changelog/cursor-projects` (404, wrong slug), and the changelog entries themselves; none of the fetched pages state a current editor version number. `src/install/vendor_cursor.rs` claims "Cursor (v2.4+)" but no primary source for today's actual version was found within budget.
- Whether Cursor's new "Security Review" custom rules (changelog: Rollouts and Security Review, 2026-09-23) share the `.cursor/rules/*.mdc` format grim renders — the changelog entry gives only a functional example ("Add rules for your codebase...") with no file path, frontmatter, or format detail, so it cannot be confirmed as the same surface grim writes to or a separate one.

Feed notes:
- Newest tag or heading seen: "Rollouts and Security Review" (https://cursor.com/changelog/rollouts-and-security-reviewer), Sep 23, 2026. Cursor found: n/a — cursor was `—`.
- Vendor version current today: not found (see Unsourced).
- Anything grim renders or documents that the feed shows changed but no ledger row covers: none of the changelog entries from 2026-07-17 to 2026-09-27 (Cursor Router, Improvements to Cursor in Slack, Cursor Start, Cursor for iPad, Google Workspace Plugins, Cloud Agents Start 3x Faster, Origin Code Hosting, Cloud Agents and Cursor Harness Improvements, Start from scratch without a repo, Self-hosted machines, Cursor Projects, Rollouts and Security Review) document a change to the `.mdc` rule format, subagent frontmatter, skill file format, `mcp.json` shape, or `CURSOR_CONFIG_DIR`/env-var scope that grim renders. Two facts current docs disclose but grim's module doc and rules do not mention (see rows 3 and 5): Cursor also reads legacy `.claude/`/`.codex/` skill and agent directories (with `.cursor/` winning name conflicts) — informational, not a behavior change, since grim already writes to `.cursor/` and wins the tie.

## Verifier sign-off

One opus verifier (C-008) re-fetched each claim that drives a docs, comment,
watchlist-wording or issue-draft change. No claim in this pass drives a
renderer, metadata or validation change unless Routing says so.

| Claim | Verdict | Evidence URL | Verbatim quote / note |
|---|---|---|---|
| V11 cursor CURSOR_CONFIG_DIR | PARTIAL | https://cursor.com/docs/cli/reference/configuration | The page does make CURSOR_CONFIG_DIR an override for the cli-config.json location: "Override with environment variables: **`CURSOR_CONFIG_DIR`**: custom directory path". The quoted sentence "controls CLI preferences and permissions only, not rules, skills, agents, or MCP paths" does NOT appear on the page or anywhere in a cursor.com search. "Relocates only" is therefore an inference, not documented. |

## Live CLI check

n/a — `sweep` depth on a Tier 2 harness (the live check is `deep` only).

## Routing

| Claim | Class | Landed as |
|---|---|---|
| 1, 2 / V11 `CURSOR_CONFIG_DIR` | (a) | watchlist row re-worded and dated (CLI config file only, IDE undocumented); `vendor_cursor.rs` module doc corrected. The researcher's "not rules, skills, agents, or MCP paths" sentence was not found by the verifier and is not relied on |
| 3, 5 extra read dirs | finding | Cursor also reads `.claude/`, `.codex/` (and `.agents/`) skill and agent dirs, `.cursor/` winning name conflicts. grim writes `.cursor/`, so nothing moves |
| 4, 6, 7 | (a) | confirmed unchanged; `vendor_cursor.rs` stamp dated |

Security: none found.

## Issue drafts

none

## Friction

- Row 1's closing quote ("controls CLI preferences and permissions only…") is not on the cited page (V11). Dropped; the row rests on the verified override sentence only.
- No Cursor editor version is published on any fetched page; the row notes the docs date instead.
- Researcher sandboxes denied network `curl` (and, for some, `gh`); the brief's "`curl -sL <url>` is fine" tip was wrong there. Researchers fell back to WebFetch, whose text is model-extracted, so quotes from vendor sites are summarizer-sourced; GitHub sources were read raw via `gh api` where allowed. Brief fixed.
- Driver cost: the brief told researchers to return only the table, so a 14-target run would have re-typed every table. This run gave each researcher one scratchpad output file and assembled the artifacts from it. Brief now names that file.

## Skill changes

One `chore(skills):` commit for the whole WP-G run, [f9effcc1](https://github.com/grimoire-rs/grimoire/commit/f9effcc1): researcher-brief scratchpad output file, no-curl fallback and re-fetch warning; domains.md docs and feed URLs (kiro, junie, amp, antigravity, goose, openclaw, qoder), `JUNIE_HOME` / `OPENCLAW_HOME` cells, section-default and stamp re-dating note; SKILL.md one artifact per target in a `--domain` run.
