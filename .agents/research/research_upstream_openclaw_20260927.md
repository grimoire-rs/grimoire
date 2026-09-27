# Research: upstream refresh — openclaw (sweep)

## Metadata
**Date:** 2026-09-27 · **Domain:** vendors · **Depth:** `openclaw sweep never checked`
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

Vendor version at check: OpenClaw **v2026.9.6** (2026-09-23, newest mainline;
`v2026.7.35` published 2026-09-21 is a patch on an older line). domains.md
named a page feed; the researcher found GH releases on `openclaw/openclaw`
with per-release `CHANGELOG/<version>.md`, which is the better feed (refined
in domains.md). Cursor was `—`, **v2026.9.6** recorded. Changelogs 2026.9.1 →
2026.9.6 swept.

## Claims

Researcher (sonnet) table, verbatim. Numbered rows re-verify ledger claims;
`I` rows are changelog or surface differences found in the sweep.

| # | Claim | Ledger location | Old value | New value | Source URL | Verbatim quote | Vendor version |
|---|---|---|---|---|---|---|---|
| 1 | Rules declined — OpenClaw's monolithic instruction files carry only `title`/`summary`/`read_when` frontmatter, no scoping key | `.claude/rules/vendor-capability-watchlist.md:183`; `src/install/vendor_openclaw.rs` (Rules bullet) | "monolithic instruction files with no in-file scoping key ... OpenClaw fixed-name files with only `title`/`summary`/`read_when`" | Partially wrong, corrected: OpenClaw's actual user-facing rule mechanism is a fixed set of workspace files (`AGENTS.md`, `SOUL.md`, `IDENTITY.md`, `USER.md`, `BOOTSTRAP.md`) injected as plain markdown — no frontmatter documented on them at all. `title`/`summary`/`read_when` is the frontmatter schema of OpenClaw's own *documentation-site pages*, not of a user-authored rule file. The "no scoping key" conclusion still holds. | https://docs.openclaw.ai/concepts/context.md | "By default, OpenClaw injects a fixed set of workspace files (if present): AGENTS.md, SOUL.md, IDENTITY.md, USER.md, BOOTSTRAP.md (first-run only)" | v2026.9.6 (docs live 2026-09-27) |
| 2 | Shared-pool membership — OpenClaw scans `$HOME/.agents/skills` at priority 3 | `.claude/rules/vendor-capability-watchlist.md:189`; `src/install/vendor_openclaw.rs` (pool bullet) | "it does scan `$HOME/.agents/skills` at priority 3, first-party confirmed" | unchanged | https://docs.openclaw.ai/tools/skills.md | "3 \| Personal agent skills \| `~/.agents/skills` (default state only)" | v2026.9.6 |
| 3 | MCP kind declined — `openclaw.json` mixes strict JSON and JSON5 (unquoted keys, `--strict-json` flag) | `.claude/rules/vendor-capability-watchlist.md:190`; `src/install/vendor_openclaw.rs` (MCP bullet) | "`openclaw.json` mixes strict JSON and JSON5 (unquoted keys, a `--strict-json` flag)" | unchanged — confirmed, and now dated: the config file is JSON5 by default, `--strict-json` is a real documented CLI flag | https://docs.openclaw.ai/cli/config ; https://github.com/openclaw/openclaw/blob/main/CHANGELOG/2026.9.1.md | "Use --strict-json to require standard JSON with no string fallback (JSON5-only syntax such as comments, trailing commas, or unquoted keys is then rejected)." / "`config set` accepts `--expect-current-json`, `--expect-current-absent`, `--dry-run`, and `--strict-json`" | v2026.9.1 (2026-09-03) |
| 4 | `$OPENCLAW_HOME` not honored / "referenced but never defined on any page fetched" | `.claude/rules/vendor-capability-watchlist.md:191`; `src/install/vendor_openclaw.rs`; brief's config-root variable note | "not honored ... referenced but never defined on any page fetched" | **Wrong, reverse the claim.** `OPENCLAW_HOME` is fully documented on its own reference page and has been honored in source since at least 2026-02-08 (commit `db137dd6`, "fix(paths): respect OPENCLAW_HOME for all internal path resolution"), well before grim's own 2026-07-27 "verified" stamp. It relocates the state dir, config path, agent dirs, credentials, installer workspace, and dev checkout. | https://docs.openclaw.ai/help/environment | "When set, OPENCLAW_HOME replaces the system home directory ($HOME / os.homedir()) for internal OpenClaw path defaults. This includes the default state directory, config path, agent directories, credentials, installer onboarding workspace, and the default dev checkout used by openclaw update --channel dev." Precedence: "OPENCLAW_HOME > $HOME > USERPROFILE > Termux PREFIX home fallback on Android > os.homedir()" | v2026.9.6 (documented; honored since well before the 2026-07-17 sweep window) |
| 5 | Skills global-scope write path: `~/.openclaw/skills/<name>/`, global scope only | `src/install/vendor_openclaw.rs`; `docs/src/content/docs/vendor-metadata.md:416` | "`~/.openclaw/skills/<name>/` — global scope only" | unchanged — confirmed via the `--global` install flag | https://docs.openclaw.ai/tools/skills.md | "Add --global to install into the shared ~/.openclaw/skills directory, visible to all local agents unless agent allowlists narrow it." | v2026.9.6 |
| 6 | Agents declined — subagents are runtime-only, nothing installable on disk | `src/install/vendor_openclaw.rs` (Agents bullet) | "Subagents are runtime-only, nothing on disk." | unchanged, no counter-evidence found: per-agent configuration lives as `agents.entries`/`agents.defaults` keys inside `openclaw.json`, not as a separate installable per-agent file format | https://docs.openclaw.ai/gateway/configuration.md | "Configuration follows a two-bucket rule: root siblings hold infrastructure and cross-agent defaults, while agents.defaults holds agent-loop behavior. Entries under agents.entries may override either bucket where the schema supports a per-agent override." | v2026.9.6 |
| 7 | Project scope resolves to a fixed `~/.openclaw/workspace` daemon home, not the invoking repository | `docs/src/content/docs/clients.md:383-385`; `docs/src/content/docs/vendor-metadata.md:389`; `src/install/vendor_openclaw.rs` | "resolves to `~/.openclaw/workspace` — a fixed daemon home that does not track the repository grim was invoked in" | unchanged | https://docs.openclaw.ai/concepts/agent-workspace | "The workspace is the agent's home: the working directory used for file tools and workspace context." / "Default: ~/.openclaw/workspace" | v2026.9.6 |
| I1 | New: `openclaw skills library` — personal skill libraries on shared Gateways, ZIP import, per-identity sharing on team Gateways | none — not covered by any current ledger row | n/a | New capability, not yet reflected anywhere in grim's OpenClaw docs/rows. Landed 2026-09-03. Touches the same "personal skills" concept as the priority-3 `~/.agents/skills` tier and the shared-pool-membership row; worth a research pass on whether it changes the pool-capability calculus. | https://github.com/openclaw/openclaw/blob/main/CHANGELOG/2026.9.1.md | "Personal skill libraries on shared Gateways: keep your own skills beside the workspace set with `openclaw skills library`, import them from ZIP archives, and share or publish them per identity on team Gateways." | v2026.9.1 (2026-09-03) |
| I2 | New (breaking): agent-owned Workshop skills — one writable Workshop collection per agent replaces workspace ownership; retires `skills.workshop.allowSymlinkTargetWrites` | none — not covered by any current ledger row | n/a | Changes the on-disk ownership model for Workshop-tier skills (priority 5, `<state-dir>/agents/<agentId>/agent/workshop-skills`). Doesn't touch the priority-4 managed path grim writes to, but is a breaking change to skill storage semantics grim may eventually need to reason about. | https://github.com/openclaw/openclaw/blob/main/CHANGELOG/2026.9.3.md | "Breaking — agent-owned Workshop skills: replace workspace ownership with one writable Workshop collection per agent and retire `skills.workshop.allowSymlinkTargetWrites`; startup and `openclaw doctor --fix` migrate proven legacy skills, while ambiguous ownership remains in place for review." | v2026.9.3 (2026-09-08) |
| I3 | New env var: `OPENCLAW_CONFIG_READONLY=1` for externally-managed, immutable `openclaw.json` | none — not covered by any current ledger row | n/a | Not in grim's env-var list for OpenClaw at all. Relevant if grim ever gains an OpenClaw config-writing path (MCP is currently declined, but this flag would need to gate any future write, the same way other vendors' read-only/managed-config markers do). | https://github.com/openclaw/openclaw/blob/main/CHANGELOG/2026.9.4.md ; https://docs.openclaw.ai/cli/config#externally-managed-config | "If another tool manages your OpenClaw settings, a new read-only configuration option keeps OpenClaw from overwriting them, including during automatic recovery. Set OPENCLAW_CONFIG_READONLY=1 for both the running service and terminal commands." | v2026.9.4 (2026-09-11) |

Unsourced:
- (none — every claim above carries a primary URL and verbatim quote; Bash `curl`/raw-page fetches were denied by the sandbox, so all fetching was done via WebFetch, per the brief's fallback instruction.)

Feed notes:
- Newest tag or heading seen: `v2026.9.6` (published 2026-09-23T23:21:10Z, via `gh api repos/openclaw/openclaw/releases`). Cursor found: n/a — cursor was `—`.
- Vendor version current today (2026-09-27): `v2026.9.6` is the newest non-prerelease tag on the mainline `9.x` line. (`v2026.7.35`, published 2026-09-21, is a later patch on an older `7.x` line, not the current head.)
- Anything grim renders or documents that the feed shows changed but no ledger row covers: see I1 (`openclaw skills library` personal libraries), I2 (Workshop skill-ownership breaking change), I3 (`OPENCLAW_CONFIG_READONLY`) above.

## Verifier sign-off

One opus verifier (C-008) re-fetched each claim that drives a docs, comment,
watchlist-wording or issue-draft change. No claim in this pass drives a
renderer, metadata or validation change unless Routing says so.

| Claim | Verdict | Evidence URL | Verbatim quote / note |
|---|---|---|---|
| V24 openclaw OPENCLAW_HOME | CONFIRMED (sub-questions answered from source) | https://docs.openclaw.ai/help/environment ; openclaw/openclaw@v2026.9.6 packages/normalization-core/src/home-dir.ts, src/infra/config-dir.ts, src/agents/workspace-default-path.ts, src/skills/loading/workspace-skill-sources.ts, src/skills/loading/skill-paths.ts | Doc: "When set, `OPENCLAW_HOME` replaces the system home directory (`$HOME` / `os.homedir()`) for internal OpenClaw path defaults. This includes the default state directory, config path, agent directories, credentials, installer onboarding workspace, and the default dev checkout used by `openclaw update --channel dev`." Skills are not mentioned. Code: `resolveConfigDir` returns `path.join(resolveRequiredHomeDir(env, homedir), ".openclaw")` and `managedSkillsDir = path.join(CONFIG_DIR, "skills")`, so skills move to `$OPENCLAW_HOME/.openclaw/skills`. The workspace becomes `path.join(home, ".openclaw", "workspace")`, so it moves to `$OPENCLAW_HOME/.openclaw/workspace`. Both hold only when STATE_DIR, CONFIG_PATH and WORKSPACE_DIR are unset. `~/.agents/skills` does NOT move: `resolveSkillsUserHomeDir()` returns `resolveOsHomeDir(...)`, which reads HOME, then USERPROFILE, then Termux, then os.homedir(), and never OPENCLAW_HOME. The personal tier is added only `if (isDefaultStateDir())`. |
| V25 openclaw releases + CONFIG_READONLY | CONFIRMED | https://api.github.com/repos/openclaw/openclaw/releases ; openclaw/openclaw@v2026.9.6 CHANGELOG/2026.9.4.md l.1174 | The newest release is v2026.9.6 (2026-09-23). v2026.7.35 (2026-09-21) is a "gateway-only `extended-stable` release ... our current equivalent to LTS", not mainline. "Set `OPENCLAW_CONFIG_READONLY=1` for both the running service and terminal commands." |

## Live CLI check

n/a — `sweep` depth on a Tier 2 harness (the live check is `deep` only).

## Routing

| Claim | Class | Landed as |
|---|---|---|
| 4 / V24 `OPENCLAW_HOME` defined | (a) + (c) | watchlist row (the old "never defined" verdict was wrong); `vendor-metadata.md` env cell; module doc; issue draft 1 |
| 1 rule frontmatter | (a) | row wording: `title`/`summary`/`read_when` is OpenClaw's docs-site schema; user rule files are fixed workspace files with no frontmatter. Decline unchanged |
| 3, I3 / V25 JSON5, `OPENCLAW_CONFIG_READONLY` | (a) | MCP-kind row dated; any future write must honor the read-only flag |
| 2, 5, 6, 7 | (a) | confirmed unchanged; stamp dated |
| I1 skill libraries, I2 Workshop skills | finding | touch the priority-5 and gateway tiers, not the managed `~/.openclaw/skills` grim writes |

Security: none found.

## Issue drafts

Filed as https://github.com/grimoire-rs/grimoire/issues/151.

### 1. OpenClaw: honor `OPENCLAW_HOME`

**Labels:** `vendor:openclaw`, `layout`

"When set, OPENCLAW_HOME replaces the system home directory ($HOME /
os.homedir()) for internal OpenClaw path defaults. This includes the default
state directory, config path, agent directories, credentials…"
(<https://docs.openclaw.ai/help/environment>, verified 2026-09-27, v2026.9.6).
grim hardcodes `~/.openclaw` (skills at `~/.openclaw/skills`, project scope
at `~/.openclaw/workspace`) on the earlier finding that the variable was
undefined. A user who sets it gets skills OpenClaw does not read. Honoring it
follows the `GEMINI_CLI_HOME` shape (replace home, keep the `.openclaw`
segment) and moves global output for those users: state migration, reaper
and upgrade fixture (Principle 9). Source at v2026.9.6 settles the
targets (V24): with `OPENCLAW_HOME` set and no `OPENCLAW_STATE_DIR` /
`OPENCLAW_CONFIG_PATH` / workspace override, managed skills move to
`$OPENCLAW_HOME/.openclaw/skills` and the workspace to
`$OPENCLAW_HOME/.openclaw/workspace`. The priority-3 `~/.agents/skills` tier
does not move (it reads the OS home) and is scanned only in the default
state dir.

## Friction

- domains.md named a docs page as OpenClaw's feed; GH releases on `openclaw/openclaw` are machine-readable, and "newest" must be by version because older lines still get patches. Fixed in domains.md.
- Researcher sandboxes denied network `curl` (and, for some, `gh`); the brief's "`curl -sL <url>` is fine" tip was wrong there. Researchers fell back to WebFetch, whose text is model-extracted, so quotes from vendor sites are summarizer-sourced; GitHub sources were read raw via `gh api` where allowed. Brief fixed.
- Driver cost: the brief told researchers to return only the table, so a 14-target run would have re-typed every table. This run gave each researcher one scratchpad output file and assembled the artifacts from it. Brief now names that file.

## Skill changes

One `chore(skills):` commit for the whole WP-G run, [f9effcc1](https://github.com/grimoire-rs/grimoire/commit/f9effcc1): researcher-brief scratchpad output file, no-curl fallback and re-fetch warning; domains.md docs and feed URLs (kiro, junie, amp, antigravity, goose, openclaw, qoder), `JUNIE_HOME` / `OPENCLAW_HOME` cells, section-default and stamp re-dating note; SKILL.md one artifact per target in a `--domain` run.
