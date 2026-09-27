# Research: upstream refresh — goose (sweep)

## Metadata
**Date:** 2026-09-27 · **Domain:** vendors · **Depth:** `goose sweep never checked`
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

Vendor version at check: Goose **v1.52.0** (2026-09-23). Feed: GH
`aaif-goose/goose` releases; cursor was `—`, **v1.52.0** recorded. The docs
moved from `block.github.io/goose` (now a redirect stub) to
<https://goose-docs.ai>, and `block/goose` redirects to `aaif-goose/goose`.

## Claims

Researcher (sonnet) table, verbatim. Numbered rows re-verify ledger claims;
`I` rows are changelog or surface differences found in the sweep.

| # | Claim | Ledger location | Old value | New value | Source URL | Verbatim quote | Vendor version |
|---|---|---|---|---|---|---|---|
| 1 | MCP kind decline reason (grim has no YAML splicer; Goose's `config.yaml` uses an `extensions:` key) | `.claude/rules/vendor-capability-watchlist.md:181` | "upstream is capable (Goose is extension/MCP-heavy) — the blocker is grim-side: its config is YAML (`config.yaml`) and grim splices JSON and TOML only" | unchanged — still YAML, still one `extensions:` map; extension `type` is now explicitly one of `builtin`, `platform`, `stdio`, `streamable_http` (SSE removed, migrate to `streamable_http`) | https://goose-docs.ai/docs/guides/config-files | "Supported extension types are `builtin`, `platform`, `stdio`, and `streamable_http`. SSE is not supported; migrate old SSE configurations to `streamable_http`." | v1.52.0 (2026-09-23, latest release) |
| 2 | macOS config root — primary config doc vs. source | `.claude/rules/vendor-capability-watchlist.md:182` | "unresolved; detection ORs `~/.config/goose/` and the macOS Application Support path; docs and source disagree" | **resolved in source's favor.** `Paths::config_dir()` (`crates/goose/src/config/paths.rs`) calls `etcetera::choose_app_strategy`, and upstream `etcetera` (pinned `0.11` in Goose's `Cargo.toml`) documents that function as XDG on macOS, not Apple's native strategy — so macOS resolves to `~/.config/goose`, matching this doc exactly | https://goose-docs.ai/docs/guides/config-files | "The primary config file is located at: macOS/Linux: `~/.config/goose/config.yaml`" | v1.52.0; etcetera 0.11 (source, `crates/goose/Cargo.toml`) |
| 3 | macOS config root — conflicting doc (GOOSE_PATH_ROOT default location table) | `.claude/rules/vendor-capability-watchlist.md:182` | (same row; this is the "Application Support" half of the disagreement) | **still contradicts row 2, upstream-side inconsistency, not resolved by grim.** `environment-variables.md` still states an Apple-style default for `GOOSE_PATH_ROOT`'s unset value, which the source code (XDG chosen on macOS per etcetera, confirmed row 2) does not actually produce | https://goose-docs.ai/docs/guides/environment-variables | "Default locations: - macOS: `~/Library/Application Support/Block/goose/`" | v1.52.0 |
| 4 | Rule kind decline reason (`.goosehints`/`AGENTS.md` monolithic, no in-file scoping key) | `.claude/rules/vendor-capability-watchlist.md:183` | "declined" — "monolithic instruction files with no in-file scoping key (Goose `.goosehints`/`AGENTS.md`...)" | unchanged. Global hints load from one file (`~/.config/goose/.goosehints`), local hints from one file per directory in the hierarchy — still no per-file `paths`-style scoping key | https://goose-docs.ai/docs/guides/context-engineering/using-goosehints | "goose supports two types of hint files: Global hints file... Global hints are stored in `~/.config/goose/.goosehints`. Local hints files - These hints will only apply when working in a specific directory or directory hierarchy." | v1.52.0 |
| 5 | Agent kind decline reason ("agents runtime- or UI-only everywhere") | `.claude/rules/vendor-capability-watchlist.md:183` | "declined" — "...agents runtime- or UI-only everywhere" | **flip condition met for Goose.** Goose now ships an installable, on-disk Markdown+YAML-frontmatter custom-agent format (`name` required; `description`, `model` optional) at `~/.agents/agents/` (global) and `<project>/.agents/agents/` (project) — shipped in [aaif-goose/goose#9293](https://github.com/aaif-goose/goose/pull/9293) "Custom agents guide", released in v1.46.0 (2026-08-12), inside the sweep window | https://goose-docs.ai/docs/guides/context-engineering/custom-agents | "Agents are stored as Markdown files with YAML frontmatter... Global agents are available across goose sessions: `~/.agents/agents/`. Project agents are available when goose is working in that project: `<project>/.agents/agents/`." | v1.46.0 (2026-08-12) onward; current v1.52.0 |
| 6 | Skills pool claim — `.goose/skills/` backward-compat, `.agents/skills` recommended | `src/install/vendor_goose.rs:9-18`; `docs/src/content/docs/vendor-metadata.md:414`; `docs/src/content/docs/clients.md:374-376` | "`.goose/skills/` directory exists but Goose's docs label it backward-compatibility, while naming `.agents/skills` the recommended location" | unchanged on the core claim; doc now also names two more backward-compat discovery paths not previously recorded: `.claude/skills/` and `~/.claude/skills/` | https://goose-docs.ai/docs/guides/context-engineering/using-skills | "goose also discovers skills from `.goose/skills/`, `.claude/skills/`, `~/.claude/skills/`, and platform-specific config directories, but `agents/skills/` is the recommended standard." | v1.52.0 |
| 7 | Docs URL / repo location cited by grim (`block.github.io/goose`, repo `block/goose`) | `src/install/vendor_goose.rs:6`; `docs/src/content/docs/clients.md:441`; `docs/src/content/docs/vendor-metadata.md:666`; `.claude/skills/upstream-refresh/references/domains.md:53` | docs URL `https://block.github.io/goose`; repo named `block/goose` | **docs domain moved to `goose-docs.ai`** (old URL still resolves but is now a redirect stub) and **the repo now lives at `aaif-goose/goose`** (`block/goose` 301-redirects there, confirmed via `gh api repos/block/goose`); the repo's own README links exclusively to `goose-docs.ai` | https://block.github.io/goose/ ; https://github.com/aaif-goose/goose/blob/main/README.md | "🦆 goose has moved! Redirecting to goose-docs.ai…" | v1.52.0 |

Unsourced:
- none — every claim above is backed by a primary URL and a verbatim quote.

Feed notes:
- Newest tag or heading seen: `v1.52.0` (published 2026-09-23). Cursor found: n/a — cursor was `—`.
- Vendor version current today: `v1.52.0`, 2026-09-23 (`gh api --paginate repos/aaif-goose/goose/releases`).
- Anything grim renders or documents that the feed shows changed but no ledger row covers:
  - Goose shipped an installable Markdown+frontmatter custom-agent format (`~/.agents/agents/`, `<project>/.agents/agents/`) in v1.46.0 (2026-08-12, [aaif-goose/goose#9293](https://github.com/aaif-goose/goose/pull/9293)/[custom-agents.md](https://goose-docs.ai/docs/guides/context-engineering/custom-agents)) — the exact flip condition the wave-2 watchlist names ("enable per vendor when a documented per-file scoping key or an installable agent file format ships"), and no ledger row records it yet.
  - The docs domain moved from `block.github.io/goose` to `goose-docs.ai`, and the GitHub org moved from `block/goose` to `aaif-goose/goose` (redirect still works, but three grim references — `vendor_goose.rs`'s module doc, `clients.md`, `vendor-metadata.md` — still cite the old docs URL, and `vendor_goose.rs` still names the old repo).
  - `documentation/docs/guides/environment-variables.md` and `documentation/docs/guides/config-files.md` disagree with each other on Goose's own macOS default path (Application Support vs. XDG) — an upstream-side inconsistency worth flagging in the ledger's "docs and source disagree" wording rather than treating it as fully resolved.

## Verifier sign-off

One opus verifier (C-008) re-fetched each claim that drives a docs, comment,
watchlist-wording or issue-draft change. No claim in this pass drives a
renderer, metadata or validation change unless Routing says so.

| Claim | Verdict | Evidence URL | Verbatim quote / note |
|---|---|---|---|
| V21 | PARTIAL | https://goose-docs.ai/docs/guides/context-engineering/custom-agents ; https://github.com/aaif-goose/goose/pull/9293 ; https://github.com/aaif-goose/goose/releases/tag/v1.46.0 | Confirmed: "Markdown files with YAML frontmatter" at `~/.agents/agents/` and `.agents/agents/`; block.github.io/goose shows "goose has moved! Redirecting to goose-docs.ai…"; `gh api repos/block/goose` resolves to `aaif-goose/goose`. Differs: [aaif-goose/goose#9293](https://github.com/aaif-goose/goose/pull/9293) is a docs PR ("docs: custom agents", body "documenting new Custom Agents feature"), not the feature itself; it is listed under Documentation in the v1.46.0 notes, but its merge commit b94b5f3 (merged 2026-06-22) is already an ancestor of tag v1.39.0 (published 2026-06-25). So "shipped v1.46.0 via #9293" is wrong on both counts. |
| V22 | CONFIRMED | https://github.com/aaif-goose/goose/blob/v1.52.0/crates/goose/src/config/paths.rs ; https://github.com/lunacookies/etcetera/blob/v0.11.0/src/app_strategy.rs ; https://github.com/aaif-goose/goose/blob/main/documentation/docs/guides/environment-variables.md | paths.rs @v1.52.0: `let strategy = choose_app_strategy(AppStrategyArgs {` (etcetera 0.11.0 per Cargo.lock). etcetera: "This uses the Windows strategy on Windows, and Xdg everywhere else." (`create_strategies!(Apple, Xdg)` on macOS). Docs: "macOS: `~/Library/Application Support/Block/goose/`". paths.rs itself carries a stale comment citing the same Library path. |

## Live CLI check

n/a — `sweep` depth on a Tier 2 harness (the live check is `deep` only).

## Routing

| Claim | Class | Landed as |
|---|---|---|
| 5 / V21 custom agents `.agents/agents/` | (a) + (c) | multi-vendor Rule + Agent row; module doc; issue draft 1 |
| 7 / V21 docs and repo moved | (a) | link refs in `clients.md`, `vendor-metadata.md`, `agents.md` and the module doc now point at `goose-docs.ai` / `aaif-goose/goose`; the catalog copy is issue draft 2 in `research_upstream_kiro_20260927.md` |
| 2, 3 / V22 macOS root | (a) | watchlist row: source says XDG (`~/.config/goose`); one upstream page still says Application Support |
| 1, 4, 6 | (a) | confirmed unchanged; stamp dated |

Security: none found.

## Issue drafts

Filed as https://github.com/grimoire-rs/grimoire/issues/147.

### 1. Goose: enable the Agent kind — file agents in `.agents/agents/`

**Labels:** `enhancement`, `vendor:goose`, `needs-design`

Goose documents agents as "Markdown files with YAML frontmatter" at
`~/.agents/agents/` (global) and `<project>/.agents/agents/` (project)
(<https://goose-docs.ai/docs/guides/context-engineering/custom-agents>,
verified 2026-09-27; documented by
[aaif-goose/goose#9293](https://github.com/aaif-goose/goose/pull/9293), merged
2026-06-22). `name` is required, `description` and `model` optional. The
watchlist's flip condition is met. The directory is a cross-vendor
`.agents/` path (Junie also scans `.agents/`), so this is the skills-pool
problem again for agents: one physical file several clients read, and
refcounting across them. Needs a design, then the usual kind-enablement
changes (grid, ADR mapping table, docs matrix, upgrade note).

## Friction

- V21 corrected the researcher: #9293 is the docs PR, merged 2026-06-22 and first shipped in v1.39.0, not "v1.46.0".
- domains.md named `block.github.io/goose` as the docs; fixed to `goose-docs.ai`.
- Researcher sandboxes denied network `curl` (and, for some, `gh`); the brief's "`curl -sL <url>` is fine" tip was wrong there. Researchers fell back to WebFetch, whose text is model-extracted, so quotes from vendor sites are summarizer-sourced; GitHub sources were read raw via `gh api` where allowed. Brief fixed.
- Driver cost: the brief told researchers to return only the table, so a 14-target run would have re-typed every table. This run gave each researcher one scratchpad output file and assembled the artifacts from it. Brief now names that file.

## Skill changes

One `chore(skills):` commit for the whole WP-G run, [f9effcc1](https://github.com/grimoire-rs/grimoire/commit/f9effcc1): researcher-brief scratchpad output file, no-curl fallback and re-fetch warning; domains.md docs and feed URLs (kiro, junie, amp, antigravity, goose, openclaw, qoder), `JUNIE_HOME` / `OPENCLAW_HOME` cells, section-default and stamp re-dating note; SKILL.md one artifact per target in a `--domain` run.
