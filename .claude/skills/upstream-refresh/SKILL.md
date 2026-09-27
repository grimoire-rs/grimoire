---
name: upstream-refresh
description: Use when grim's upstream knowledge may be stale — a harness shipped or changed a capability, watchlist rows past six months, vendor changelogs since the last check, a deep re-verify of one harness (claude, codex, copilot…), spec or forge claims, or expired research.
user-invocable: true
disable-model-invocation: false
argument-hint: "[<harness>] [--domain <domain>] [--force] [--focus \"<text>\"]"
triggers:
  - "upstream refresh"
  - "upstream freshness"
  - "re-verify upstream"
  - "harness capability sweep"
  - "vendor changelog check"
  - "stale watchlist rows"
---

# /upstream-refresh — keep grim's upstream facts current

Re-checks the dated upstream claims grim depends on — harness capabilities,
metadata keys, config paths and env vars, specs, forge APIs, the product
landscape, research expiry. Check ledger:
[`.agents/upstream-checks.md`](../../../.agents/upstream-checks.md); per-domain
registry: [references/domains.md](references/domains.md). Design record:
[harness-capability-freshness.md](../../../.agents/discussions/harness-capability-freshness.md).

## Modes

```
/upstream-refresh [<harness>] [--domain <domain>] [--force] [--focus "<text>"]
```

| Invocation | Targets | Depth |
|---|---|---|
| `/upstream-refresh` | every non-`noop` ledger row | per ladder, never `deep` |
| `/upstream-refresh <harness>` | that harness row | `deep` (the only way to get one) |
| `--domain vendors` | the 18 `harness` rows | per ladder, never `deep` |
| `--domain <catalog\|specs\|forge\|landscape\|research>` | that one `domain` row | per ladder |
| `--force` | as selected | a `noop` becomes `sweep` |
| `--focus "<surface>"` | every harness row, or only the named harness | that surface only, ladder ignored |

**Validate first.** The positional token must be a harness name from the
ledger (`claude`, `opencode`, `copilot`, `codex`, `cursor`, `kiro`, `junie`,
`gemini`, `zed`, `amp`, `antigravity`, `cline`, `droid`, `goose`, `warp`,
`openclaw`, `kilo`, `qoder`); `--domain` must be `vendors` or a domain row
name. Anything else → stop **before any edit**, print the valid names. An
unknown token is never read as a focus.

## Depth ladder

Never compute ages by hand:

```sh
python3 .claude/skills/upstream-refresh/scripts/upstream_stale.py ladder \
  [--name <harness>] [--domain <domain>] [--force]
```

It prints `<target> <noop|feed|sweep|deep> <reason>` per target. Exit 2 →
relay its stderr and stop. `0 days` → `noop`; `1–2 days` → `feed` (read the
change feed from the cursor); older or never checked → `sweep`; a named
harness → `deep`. A reason ending `deep due` → name that harness in the
report as due for `/upstream-refresh <harness>`. Every target
`noop` → print `every target checked today — no-op (use --force)`, edit
nothing. `task upstream:stale` (the script's `report`) is the overview.

## Default run

Dead simple, one sitting (1–2 hours):

1. **Preflight.** Not on `main`; honor `hex-state.md`. Read the ledger and
   [domains.md](references/domains.md).
2. **Pick** every non-`noop` target from the ladder.
3. **Fan out — everything at once.** One sonnet update-check subagent per
   target: every Tier 1 and Tier 2 harness and every domain in the pick,
   spawned together, no tier ordering, none waiting on another. Each is
   briefed from
   [researcher-brief.md](references/researcher-brief.md) with its depth
   (`feed` → Feed procedure; `sweep` → every ledger row and code stamp the
   target owns). `catalog` is confirm-only and `research` follows its dating
   rule (domains.md). A silent subagent is retried once, then its row stays
   unchanged with a note.
4. **Collect.** Findings into `## Claims`; drop any without a URL and a
   verbatim quote (list it under `## Friction`).
5. **Write** class (a) fixes (Routing): ledger dates with evidence, doc and
   matrix date fixes. Update each completed target's ledger row —
   `Last check` = today, `Depth`, `Feed cursor` (the feed's newest tag),
   `Notes`. A target that did not complete keeps its row and stays stale.
6. **Report** code-driving findings (renderer, metadata, validation) in the
   run report and the artifact's `## Routing` — they do not block the run.
7. **Refine and commit** (Refinement, Commits).

No per-target opus gate, no per-target verify, no ordering of research —
only the ledger writes and code changes after collection serialize.

**Landing a code change** (only when this run implements one): one batched
opus verifier re-fetches the sources of every code-driving claim and signs
off or refutes in `## Verifier sign-off` — refuted claims do not land. Then
implement per Routing (b) with tests and run **one** `task verify` before the
final commit.

**Deep pass** (named harness only): the default run plus
[deep-pass.md](references/deep-pass.md) — full surface inventory and the
sandboxed live CLI check.

### Feed procedure (`feed` rung)

1. Feed and cursor come from domains.md and the ledger row.
2. **GitHub feed** — the paginated Releases API, never `releases.atom`:
   `gh api --paginate repos/<o>/<r>/releases --jq '.[] | [.tag_name, .prerelease] | @tsv'`,
   filtered by the tag stream domains.md names; entries above the cursor are
   new. **Page feed** (changelog) — headings above the cursor are new.
3. Cursor not in the fetched window → escalate to `sweep`, record friction.
   Feed unreachable → keep the cursor, note it, go on.
4. Relevant entries touch a surface grim renders or documents (skills, rules,
   agents, MCP, hooks, frontmatter keys, config paths, env vars); others are
   skipped. Advance the cursor to the newest tag, heading or version seen.

## Routing

| Class | What | How it lands |
|---|---|---|
| **(a)** | Ledger dates, docs text, matrix rows | Fixed directly, `chore:` or `docs:` commit |
| **(b)** | Additive renderer or metadata support | One commit: renderer + docs page + parity test + watchlist row, with tests |
| **(c)** | Compatibility; config-path or install-layout moves; new vendors; class-3 requests | Issue draft — never code |
| **(c) security** | Any security-relevant finding | `(c) security — <file:line> · <class>` in `## Routing` only — no draft, no detail |

**(b) rules** (Principle 9, `docs/src/content/docs/stability.md`): additive
only; a self-heal test re-materializes an artifact installed before the
change and asserts `grim status` reports `state == "installed"`. Both
`src/install/client_target.rs` parity tests must pass — red → revert and
draft an issue, never weaken the test. A docs-page change triggers the
catalog drift review (`catalog/README.md`). Hooks
([PR #98](https://github.com/grimoire-rs/grimoire/pull/98)) are findings only.

**Issues.** `gh issue create` only under an issue-creation grant; otherwise
an issue draft (title, body, labels) under `## Issue drafts`, listed in the
final report. Security findings are never drafted as issues.

## Evidence

Every changed or newly dated ledger row carries `verified YYYY-MM-DD`, a
primary-source URL, and the vendor version when known. Each target writes
`.agents/research/research_upstream_<target>_<YYYYMMDD>.md`,
`**Expires:**` = today + 6 months:

```markdown
# Research: upstream refresh — <target> (<depth>)

## Metadata
**Date:** <YYYY-MM-DD> · **Domain:** <domain> · **Depth:** <ladder line>
**Triggered by:** /upstream-refresh <args>
**Expires:** <today + 6 months, YYYY-MM-DD>

## Claims            <!-- researcher-brief.md output table -->
## Verifier sign-off <!-- only when code lands; else "none required" -->
## Live CLI check    <!-- deep only: command · pass|fail|skip · reason -->
## Routing           <!-- claim → a|b|c → commit or draft; security: (c) security — <file:line> · <class> -->
## Issue drafts      <!-- or "none" -->
## Friction          <!-- dropped claims, feed escalations, retries -->
## Skill changes     <!-- summary + commit, or "none" -->
```

## Refinement

Friction with **this** skill is fixed in the same run: record it under
`## Friction`, fix `.claude/skills/upstream-refresh/**` in its own
`chore(skills):` commit, and append one line to the discussion's
`## Hand-off record`:
`- <date> · <target> · <depth> · skill: <change summary | no change> · artifact: <path>`.
Friction with any other skill or rule goes to the retro inbox only
(`hex-state.md`).

## Commits

- Conventional Commits with `--signoff` on the current branch — never `main`,
  never pushed. `chore:` for ledger, research and skill commits; `feat:` /
  `fix:` / `docs:` for renderer and docs changes.
- A pure ledger/doc run gates on `task claude:verify` (plus `task docs:check`
  when a docs page changed); if the commit hook still demands a fresh
  `task verify`, run it once for the whole run. A run that lands code runs
  `cargo fmt` and one `task verify` before its final commit.
