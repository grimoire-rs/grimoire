# Research: upstream refresh — openclaw (sweep)

## Metadata
**Date:** 2026-11-16 · **Domain:** vendors · **Depth:** sweep — checked 50d ago
**Triggered by:** /upstream-refresh
**Expires:** 2027-05-16

## Claims

| # | Claim | Ledger location | Old value | New value | Source URL | Verbatim quote | Vendor version |
|---|---|---|---|---|---|---|---|
| 1 | `OPENCLAW_HOME` replaces the home directory; `OPENCLAW_STATE_DIR` outranks it; state dir default `~/.openclaw` | .agents/upstream-watchlist.md:133 | honored; root = `$OPENCLAW_STATE_DIR`, else `$OPENCLAW_HOME/.openclaw`, else `~/.openclaw` | unchanged | https://docs.openclaw.ai/help/environment | "When set, `OPENCLAW_HOME` replaces the system home directory (`$HOME` / `os.homedir()`) for internal OpenClaw path defaults." / "Explicit path variables such as `OPENCLAW_STATE_DIR`, `OPENCLAW_CONFIG_PATH`, and `OPENCLAW_GIT_DIR` still take precedence." / "Override the state directory (default `~/.openclaw`)." | live docs, newest release v2026.11.4 |
| 2 | Upstream expands a leading `~` in both variables | .agents/upstream-watchlist.md:133 | expands `~` in both; other relative values ignored | unchanged (the "other relative ignored" half is grim-side; upstream says only STATE_DIR/CONFIG_PATH/HOME expand `~`) | https://docs.openclaw.ai/help/environment | "`OPENCLAW_STATE_DIR` and `OPENCLAW_CONFIG_PATH` do expand `~`." / "`OPENCLAW_HOME` can also be set to a tilde path (e.g. `~/svc`), which gets expanded using the same OS home fallback chain before use." | live docs, v2026.11.4 |
| 3 | Managed skills at `<state-dir>/skills`; `~/.agents/skills` tier is priority 3, "default state only" | .agents/upstream-watchlist.md:131, :133 | priority 3, default state only | unchanged | https://docs.openclaw.ai/tools/skills.md | "\| 3           \| Personal agent skills  \| `~/.agents/skills` (default state only)              \|" / "\| 4           \| Managed / local skills \| `<state-dir>/skills`                                 \|" | live docs, v2026.11.4 (same table at tag v2026.9.6) |
| 4 | A non-default `OPENCLAW_STATE_DIR` drops the `~/.agents/skills` root | .agents/upstream-watchlist.md:133, src/install/vendor_openclaw.rs:156-159 | pool not moved by either variable | unchanged | https://docs.openclaw.ai/tools/skills.md | "When `OPENCLAW_STATE_DIR` points somewhere other than the default `~/.openclaw`, session skill indexes exclude home-scoped personal or compatibility skill roots such as `~/.agents/skills`." | live docs, v2026.11.4 |
| 5 | Shared-pool membership: OpenClaw reads `~/.agents/skills` at priority 3, global only; new 2026.9.1/2026.9.3 tiers | .agents/upstream-watchlist.md:131, src/install/vendor_openclaw.rs:28 | NOT pool-capable, deliberate deferral | unchanged | https://docs.openclaw.ai/tools/skills.md | "\| 5           \| Workshop skills        \| `<state-dir>/agents/<agentId>/agent/workshop-skills` \|" | live docs, v2026.11.4 |
| 6 | MCP kind declined: `openclaw.json` is JSON5 by default, `--strict-json` flag | .agents/upstream-watchlist.md:132, src/install/vendor_openclaw.rs:40-43, docs/src/content/docs/mcp-servers.md:239-241 | JSON5 default; strict flag exists | unchanged | https://docs.openclaw.ai/cli/config | "Values parse as JSON5 when possible; otherwise they are treated as raw strings. Use `--strict-json` to require standard JSON with no string fallback" | live docs, v2026.11.4 |
| 7 | `openclaw.json` is JSON5 on disk (re-check trigger: OpenClaw documents plain-JSON `openclaw.json`) | .claude/skills/upstream-refresh/references/domains.md:112 | trigger not fired | unchanged, trigger has not fired (grim has no JSON5 splice engine, unchanged) | https://docs.openclaw.ai/gateway/configuration | "OpenClaw reads an optional <Tooltip tip=\"JSON5 supports comments and trailing commas\">**JSON5**</Tooltip> config from `~/.openclaw/openclaw.json`." | live docs, v2026.11.4 |
| 8 | `OPENCLAW_CONFIG_READONLY=1` marks the file externally managed (2026.9.4); any future write must honor it | .agents/upstream-watchlist.md:132 | future write must honor | unchanged | https://docs.openclaw.ai/cli/config | "When `OPENCLAW_CONFIG_READONLY=1` or `OPENCLAW_NIX_MODE=1`, OpenClaw treats `openclaw.json` as immutable." | live docs, v2026.11.4 |
| 9 | Rules declined: fixed workspace files, no scoping key | .agents/upstream-watchlist.md:120, src/install/vendor_openclaw.rs:35-38 | no per-file scoping key | unchanged | https://docs.openclaw.ai/concepts/agent-workspace.md | "`openclaw onboard`, `openclaw configure`, or `openclaw setup` create the workspace and seed the bootstrap files if they are missing." (SOUL.md, IDENTITY.md, BOOTSTRAP.md each listed as a fixed workspace file) | live docs, v2026.11.4 |
| 10 | Agents declined: settings-file only, nothing per-file on disk | .agents/upstream-watchlist.md:120, src/install/vendor_openclaw.rs:39 | settings-file only | unchanged (agents live under `agents.entries` in `openclaw.json`; no per-file format found in tools/skills, configuration or workspace docs) | https://docs.openclaw.ai/gateway/configuration | "Entries under `agents.entries` may override e" (line truncated in my capture; full sentence not re-fetched) | live docs, v2026.11.4 |
| 11 | Default workspace `~/.openclaw/workspace` is a fixed daemon home; no per-repository scope (project scope declined) | docs/src/content/docs/clients.md:605-607, vendor-metadata.md:500, src/install/vendor_openclaw.rs:15-17 | fixed daemon home, global only | unchanged for the default workspace; see Feed notes for a new Control UI "registered project" skill source | https://docs.openclaw.ai/concepts/agent-workspace.md | "Default: `~/.openclaw/workspace`" / "A non-default `OPENCLAW_STATE_DIR` keeps the default workspace at `<state-dir>/workspace`" | live docs, v2026.11.4 |
| 12 | vendor-metadata.md:500 path table: global skills at `$OPENCLAW_STATE_DIR/skills`, else `$OPENCLAW_HOME/.openclaw/skills`, `.openclaw` segment still appended | docs/src/content/docs/vendor-metadata.md:500 | as stated | unchanged | https://docs.openclaw.ai/help/environment | "`OPENCLAW_HOME` replaces the system home directory" for "the default state directory, config path, agent directories" (state default is `~/.openclaw`, so `.openclaw` is appended) | live docs, v2026.11.4 |
| 13 | Code honors `OPENCLAW_STATE_DIR` and `OPENCLAW_HOME`: domains.md:67 says `OPENCLAW_HOME` "not honored", AGENTS.md says honored | .claude/skills/upstream-refresh/references/domains.md:67 vs AGENTS.md env table | domains.md: "not honored; documented, replaces home" | **AGENTS.md is true, domains.md:67 is stale.** `openclaw_root` reads `OPENCLAW_STATE_DIR` first, else `OPENCLAW_HOME`+`.openclaw`, else `home_dir()`; used for Global scope at vendor_openclaw.rs:95,106,148. Fix domains.md:67 (class a) | src/install/vendor_openclaw.rs:45-46,166-175 (repo code, not upstream) | `//! `$OPENCLAW_STATE_DIR` and `$OPENCLAW_HOME` **are** honored for the global` / `//! root, state dir first ([`openclaw_root`]).` | n/a |

## Verifier sign-off

none required — no code landed in this run.

## Routing

- #2 Upstream expands a leading ~ in both variables → (a) ledger or doc text fix
- #7 openclaw.json is JSON5 on disk (re-check trigger: OpenClaw documents plain-JSON... → (a) ledger or doc text fix
- #10 Agents declined: settings-file only, nothing per-file on disk → (a) ledger or doc text fix
- #11 Default workspace ~/.openclaw/workspace is a fixed daemon home; no... → (a) ledger or doc text fix
- #13 Code honors OPENCLAW_STATE_DIR and OPENCLAW_HOME: domains.md:67 says... → (a) ledger or doc text fix

## Issue drafts

none

## Friction

- dropped (unsourced): Rename lineage claim (`metadata.clawdbot` / `metadata.clawdis` legacy aliases) at src/install/vendor_openclaw.rs:6-11: not re-checked in this sweep, no primary quote fetched.
- dropped (unsourced): Hook kind "declined, JS/TS plugin module" (.agents/upstream-watchlist.md:236, :255 and code line 875): not fetched; only saw configuration doc listing "cron, hooks" and a link to https://docs.openclaw.ai/plugins/hooks.
- dropped (unsourced): Row 10 quote is a truncated capture; counts as weakly sourced, driver may drop it.
- not reached: docs/src/content/docs/settings.md (no OpenClaw statements found by grep).
- not reached: Full read of 2026.9.7.md and 2026.10.5/2026.10.10.md (cumulative 0.5-0.8 MB files); only keyword greps were run.
- feed: newest v2026.11.4 (2026-11-16), cursor found yes

## Skill changes

Host caps concurrent subagents at 20; the researcher built its own claim list; replies are a short summary with the table in a file. Fixed in this run's chore(skills) commit.
