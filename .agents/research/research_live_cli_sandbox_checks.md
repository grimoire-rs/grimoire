# Research: Headless, no-auth live CLI checks for the four Tier-1 harnesses

## Metadata

**Date:** 2026-09-27
**Domain:** cli
**Triggered by:** upstream-refresh skill's deep-pass procedure — which harnesses get a live parse check
**Expires:** 2027-03-27

## Direct Answer

All four Tier 1 harnesses (Claude Code, Codex CLI, OpenCode, Copilot CLI) ship
at least one config-listing subcommand that runs with no vendor login, under
a throwaway `HOME`/config-root env var, and confirms the harness actually
*parsed* grim-rendered config rather than merely that files exist on disk.
Feasibility is **yes for three, partial for Claude Code** — `claude mcp list`
always performs live network health-checks against five bundled default
remote connectors (`api.anthropic.com`, `elevenlabs.io`, `mcp.notion.com`,
`chatgpt.mermaid.ai`, `drivemcp.googleapis.com`) regardless of `HOME`, so it
is not silent even though it needs no credentials and never touches
grim-rendered servers beyond showing them as "Pending approval" (unapproved
project `.mcp.json` entries are never connected to). OpenCode was not locally
installed; its verdict rests on vendor docs only.

Bonus finding from the live probe: `copilot skill list`, run against this
repo's real `.claude/skills/`, rejected five real skills
(`finalize`, `meta-maintain-config`, `next`, `qa-engineer`,
`security-auditor`) with "failed to parse YAML frontmatter" — a concrete
demonstration that a live parse check catches drift a file-existence check
would miss.

## Verdict table

| Harness | Install | No-auth list command | Config-root env var | Live check feasible |
|---|---|---|---|---|
| Claude Code | `npm i -g @anthropic-ai/claude-code` or native installer; headless | `claude mcp list` (MCP), no direct skill/agent list subcommand found | `CLAUDE_CONFIG_DIR` | **Partial** — command itself needs no login and correctly renders grim's own MCP entries as Pending/Connected without touching them, but it always health-checks 5 bundled default connectors over the network first; no flag found to suppress that. Skills have no listing subcommand at all — only indirect evidence via `/skill-name` at runtime or `claude doctor` (which also attempts an org-resolution network call and reports nothing about skills). |
| Codex CLI | `npm i -g @openai/codex` or install script; headless | `codex mcp list`, `codex mcp get <name>` | `CODEX_HOME` (dir must already exist) | **Yes** — verified locally: fully offline, no auth, correctly reports a configured stdio server's command/args/status. One caveat: refuses to create PATH-alias helper binaries when `CODEX_HOME` sits directly under `/tmp` (warns, does not fail) — use a scratch dir one level deeper. |
| OpenCode | `npm i -g opencode-ai` or `curl -fsSL https://opencode.ai/install \| bash`; headless | `opencode agent list`, `opencode mcp list`, `opencode debug`, `opencode session list` — all documented offline | `OPENCODE_CONFIG_DIR` (dir) / `OPENCODE_CONFIG` (file) | **Yes, per docs only** — not installed locally, so unverified by direct probe. `opencode models`/`opencode auth list` need prior provider credentials; the listing/debug commands do not. |
| Copilot CLI | `npm i -g @github/copilot`; headless (login only prompted on first interactive launch) | `copilot skill list`, `copilot instruction list`, `copilot mcp list`, `copilot lsp list` | `COPILOT_HOME` | **Yes** — verified locally: all four ran with no login, no network activity observed, and correctly read a fake `COPILOT_HOME`-relative personal skill plus real project skills/instructions from cwd. `copilot mcp list` reports one always-present builtin (`github-mcp-server`, http) alongside grim-rendered entries. |

## Findings

1. **Claude Code MCP listing is honest about approval state but not silent.**
   `claude mcp list --help`: "Unapproved `.mcp.json` servers are shown as ⏸
   Pending approval and not connected to; approved servers are
   health-checked unless disabled for this project." Verified locally: a
   demo project-scope `.mcp.json` entry showed `Pending approval` and was
   never dialed, while five hardcoded default remote connectors
   (`claude.ai Claude Docs`, `ElevenLabs`, `Notion`, `Mermaid Chart`, `Google
   Drive`) were health-checked over the network and reported `✔ Connected`
   even with `HOME` pointed at an empty `mktemp -d`. These five are shipped
   defaults, not something grim renders, so they don't invalidate the check
   for grim's own MCP entries — but the command is not network-silent.
   [`claude mcp` help, local `claude --version 2.1.283`]
2. **Claude Code has no CLI subcommand that lists skills or rules.**
   `claude --help` Commands section (local, v2.1.283): `agents`, `attach`,
   `auth`, `auto-mode`, `doctor`, `gateway`, `import`, `install`, `logs`,
   `mcp`, `plugin`, `project`, `respawn`, `rm`, `setup-token`,
   `stop`/`kill`, `ultrareview` — no `skills` or `rules` verb. `claude
   doctor` "Reads settings files in the current directory without a trust
   prompt" but only reports installation health (PATH, update channel,
   Remote Control org resolution — which itself attempted an org-lookup
   network call and failed gracefully), not skill/rule parse status.
   Skill/rule validity can only be probed indirectly (e.g. `/agents` inside
   an interactive session, out of scope for headless).
3. **Env vars to quiet Claude Code:** `CLAUDE_CONFIG_DIR` relocates the
   config root; `DISABLE_TELEMETRY`, `DISABLE_ERROR_REPORTING`,
   `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC` disable telemetry/error
   reporting/non-essential traffic (presence-only booleans — unset or empty
   to turn back on). No documented var disables the MCP default-connector
   health check specifically.
   [code.claude.com/docs/en/env-vars](https://code.claude.com/docs/en/env-vars)
4. **Codex CLI's `mcp list`/`mcp get` are clean, offline, no-auth.** Verified
   locally with a real `[mcp_servers.demo]` entry in a sandboxed
   `CODEX_HOME/config.toml`: `codex mcp list` printed a table
   (Name/Command/Args/Env/Cwd/Status/Auth) with zero network calls and exit
   0. `CODEX_HOME` "Sets the root for Codex state, including config, auth,
   logs, sessions, skills, and standalone package metadata" — covers skills
   too, though no `codex skills list` subcommand was found in `--help`
   (skills load implicitly; only `codex mcp`/`codex plugin`/`codex features`
   are inspectable non-interactively).
   [learn.chatgpt.com/codex/config-file/environment-variables](https://learn.chatgpt.com/codex/config-file/environment-variables)
5. **Codex refuses PATH-alias helpers directly under `/tmp`.** Every sandboxed
   run printed: `WARNING: proceeding, even though we could not create PATH
   aliases: Refusing to create helper binaries under temporary dir "/tmp"`.
   Non-fatal (exit 0, functionality unaffected) but noisy; a deep-pass
   script should either accept the warning as benign or nest `CODEX_HOME`
   one directory below `/tmp` root to silence it. [local probe]
6. **OpenCode's headless-safe subcommands, per official docs:**
   `opencode agent list`, `opencode mcp list`, `opencode mcp debug <name>`
   (OAuth troubleshooting), `opencode session list`, `opencode debug`. These
   need no provider credentials; `opencode models` and `opencode auth list`
   do (they read from configured/authenticated providers).
   [opencode.ai/docs/cli](https://opencode.ai/docs/cli/),
   [opencode.ai/docs/config](https://opencode.ai/docs/config/)
7. **OpenCode config root:** `OPENCODE_CONFIG_DIR` (directory, searched
   "just like the standard `.opencode` directory" — additive, for
   agents/commands/modes/plugins) or `OPENCODE_CONFIG` (a specific file
   path). Docs do not document a telemetry/update-check disable env var —
   only the `autoupdate` key inside `opencode.json`.
   [opencode.ai/docs/config](https://opencode.ai/docs/config/)
8. **Copilot CLI's inspect subcommands are purpose-built for exactly this
   check and ran clean locally with no login.** `copilot skill list` (with
   `COPILOT_HOME` pointed at a sandbox dir) correctly showed a personal
   skill placed at `$COPILOT_HOME/skills/demo/SKILL.md`, plus this repo's
   real project skills from `.claude/skills/` and `.github/skills/`/
   `.agents/skills/` conventions — and surfaced five genuine YAML
   frontmatter parse failures (`finalize`, `meta-maintain-config`, `next`,
   `qa-engineer`, `security-auditor`: "failed to parse YAML frontmatter:
   mapping values are not allowed in this context"). `copilot instruction
   list` listed `AGENTS.md`/`CLAUDE.md` as discovered sources. `copilot lsp
   list` and `copilot mcp list` (showing a builtin `github-mcp-server` plus
   any configured servers) both exited 0 with no network activity observed.
   [local probe, `copilot --version 1.0.88`]
9. **Copilot CLI install is headless; login is deferred to first
   interactive launch**, not to installation or to any inspect subcommand:
   "On first launch, if you're not currently logged in to GitHub, you'll be
   prompted to use the `/login` slash command" — that prompt never fired
   during any of the four non-interactive `list` probes.
   [docs.github.com/en/copilot/how-tos/set-up/install-copilot-cli](https://docs.github.com/en/copilot/how-tos/set-up/install-copilot-cli)
10. **`COPILOT_HOME`** is grim's own documented, already-honored relocation
    var for Copilot skills/agents (`AGENTS.md` env table) and was confirmed
    live: personal skills placed under `$COPILOT_HOME/skills/` were picked
    up correctly by `copilot skill list`.

## Negative

- No official Copilot CLI env var for disabling telemetry/network calls was
  confirmed on a primary source; a community mention of `COPILOT_OFFLINE`
  surfaced in search results but could not be verified against
  `docs.github.com` or the `github/copilot-cli` repo, so it is **not**
  reported as a fact here.
- No subcommand on any of the four harnesses gives a single "validate all
  config" verdict; each covers one category (MCP, skills, instructions,
  agents) and the deep-pass procedure needs one invocation per category.
- OpenCode has no local install in this environment — all OpenCode rows are
  docs-only, unverified by direct probe. If the deep-pass procedure needs
  parity of confidence with the other three, budget time to `npm i -g
  opencode-ai` in CI and probe once.

## Recommendation

Run all four as part of the deep pass, each under an isolated
`HOME`/config-root env var (never the owner's real `$HOME`):

- `CODEX_HOME=<scratch>/.codex codex mcp list` — clean, no caveats beyond
  nesting the dir below `/tmp`'s root to silence the alias warning.
- `COPILOT_HOME=<scratch>/.copilot copilot skill list && copilot instruction
  list && copilot mcp list && copilot lsp list` — clean, and the highest-value
  check of the four (it validates skill frontmatter, not just presence).
- `CLAUDE_CONFIG_DIR=<scratch>/.claude claude mcp list` (project dir with a
  grim-rendered `.mcp.json`) — accept the network health-check of the 5
  bundled default connectors as expected noise; treat only the
  grim-rendered entries' Pending/Connected line as the signal. Note in the
  procedure that skill/rule parsing has no CLI check for Claude Code today.
- `OPENCODE_CONFIG_DIR=<scratch>/.opencode opencode agent list && opencode
  mcp list` — expect this to work per docs; note as unverified-by-probe
  until run once for real.

## Sources

| Source | Type | Date | Relevance |
|--------|------|------|-----------|
| Local probe: `claude --help`, `claude mcp --help`, `claude mcp list`, `claude doctor` (v2.1.283) | CLI output | 2026-09-27 | Subcommand inventory, MCP approval/health-check behavior |
| [code.claude.com/docs/en/env-vars](https://code.claude.com/docs/en/env-vars) | Vendor docs | 2026-09-27 | `CLAUDE_CONFIG_DIR`, `DISABLE_TELEMETRY`, `DISABLE_ERROR_REPORTING`, `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC` |
| Local probe: `codex --help`, `codex mcp --help`, `codex mcp list`, `codex mcp get` (v0.153.4) | CLI output | 2026-09-27 | MCP list/get behavior, `/tmp` alias-helper warning |
| [learn.chatgpt.com/codex/config-file/environment-variables](https://learn.chatgpt.com/codex/config-file/environment-variables) | Vendor docs | 2026-09-27 | `CODEX_HOME` scope (config, auth, logs, sessions, skills) |
| [opencode.ai/docs/cli](https://opencode.ai/docs/cli/) | Vendor docs | 2026-09-27 | `agent list`, `mcp list`, `mcp debug`, `session list`, `debug`, offline-vs-auth split |
| [opencode.ai/docs/config](https://opencode.ai/docs/config/) | Vendor docs | 2026-09-27 | `OPENCODE_CONFIG_DIR`, `OPENCODE_CONFIG`, no documented telemetry-disable var |
| Local probe: `copilot --help`, `copilot skill/instruction/lsp/mcp list` (v1.0.88) | CLI output | 2026-09-27 | Skill frontmatter validation, instruction discovery, no-login behavior |
| [docs.github.com/en/copilot/how-tos/set-up/install-copilot-cli](https://docs.github.com/en/copilot/how-tos/set-up/install-copilot-cli) | Vendor docs | 2026-09-27 | Login deferred to first interactive launch, not install |
| `AGENTS.md` (this repo) | Internal | 2026-09-27 | `COPILOT_HOME`, `CODEX_HOME`, `CLAUDE_CONFIG_DIR`, `OPENCODE_CONFIG_DIR` already-honored by grim install |
