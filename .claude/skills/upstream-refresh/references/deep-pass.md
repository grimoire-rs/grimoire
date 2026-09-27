# Deep pass

A `deep` target gets everything a `sweep` does, plus the two steps below.
Deep applies only to a harness named at invocation; the ladder marks an
unnamed Tier 1 row whose `Last deep` is 30+ days old or absent as
`sweep … deep due`.

## 1. Full surface inventory

Diff the vendor's **entire current** config surface against grim — not only
the stale watchlist rows. The researcher lists, from primary sources:

| Surface | Compare against |
|---|---|
| Artifact kinds (skills, rules or instructions, agents, MCP, hooks, commands) | the harness's `KindSupport` row in `src/install/client_target.rs` and `docs/src/content/docs/clients.md` |
| Frontmatter and metadata keys per kind | the vendor registry in `src/install/vendor_<harness>.rs` and `docs/src/content/docs/vendor-metadata.md` |
| MCP options (transports, env refs, oauth, headers) | the vendor's MCP projection and `docs/src/content/docs/mcp-servers.md` |
| Config paths per scope | the renderer's roots and `.claude/rules/subsystem-file-structure.md` |
| Env vars (config roots, overrides) | the AGENTS.md env table and the watchlist's env-var rows |
| Hooks | finding only — the hooks artifact kind ([PR #98](https://github.com/grimoire-rs/grimoire/pull/98)) is unmerged and out of scope for the renderer |

Every difference becomes a claim in the researcher's table and is routed
like any other (SKILL.md › Routing). A new kind, key or path is at most
class (b); a moved path is class (c).

## 2. Live CLI check

Confirms the harness actually **parses** what grim renders, not merely that
files exist. Never with the owner's credentials, never against the owner's
real `HOME`.

### Sandbox recipe

```sh
REPO=<absolute repo root>                             # the branch under test
(cd "$REPO" && cargo build --release)                 # test this branch's renderer, not an installed grim
SCRATCH=<session scratchpad>/upstream-<harness>      # never directly under /tmp
mkdir -p "$SCRATCH"
SB=$(mktemp -d "$SCRATCH/sb.XXXXXX")                 # HOME nests one level below scratch
mkdir -p "$SB/home" "$SB/npm" "$SB/grim" "$SB/project" "$SB/fixtures"

# Install — the only step that needs registry network access.
npm install --ignore-scripts --prefix "$SB/npm" <npm package>

# MCP has no path source (`grim add <toml> --kind mcp` exits 64), so the
# descriptor goes through a throwaway loopback registry. Stop it at the end.
docker run -d --rm --name grim-upstream-<harness> -p 127.0.0.1:5077:5000 registry:2

# Render fixtures with grim, sandboxed too.
cd "$SB/project"
# Every grim add path is absolute: add treats an argument as a path only
# when it starts with ./, ../ or /.
env -i PATH="$REPO/target/release:$PATH" HOME="$SB/home" GRIM_HOME="$SB/grim" \
  GRIM_INSECURE_REGISTRIES=localhost:5077 REPO="$REPO" SB="$SB" \
  <root var>="<root path>" sh -c '
    grim init &&
    grim add "$REPO/catalog/skills/grim-usage" --no-install &&
    grim add "$SB/fixtures/rule.md" --kind rule --no-install &&
    grim add "$SB/fixtures/agent.md" --kind agent --no-install &&
    grim release --kind mcp "$REPO/catalog/mcp/grim.toml" localhost:5077/fixture/grim:1 &&
    grim add localhost:5077/fixture/grim:1 --kind mcp --no-install &&
    grim install --client <harness>'

# Check — no inherited credentials, telemetry off.
env -i PATH="$SB/npm/node_modules/.bin:$PATH" HOME="$SB/home" \
  <root var>="<root path>" DO_NOT_TRACK=1 <vendor opt-outs> \
  <list command>
```

- `env -i` with only `PATH` means no `ANTHROPIC_API_KEY`,
  `CLAUDE_CODE_OAUTH_TOKEN`, `GH_TOKEN`, `GITHUB_TOKEN`, `OPENAI_API_KEY` or
  other token reaches the CLI.
- `--ignore-scripts` is mandatory. A CLI that cannot run without its
  install scripts is `skip` with that reason.
- **Fixtures:** one skill (`$REPO/catalog/skills/grim-usage`), one MCP
  server (`$REPO/catalog/mcp/grim.toml`), and a minimal rule and agent
  (`$SB/fixtures/rule.md`, `$SB/fixtures/agent.md`). Pass `--kind agent`
  explicitly (a bare `.md` is inferred as a rule) and `--kind rule` for the
  rule. The MCP descriptor needs `--kind mcp` on `grim release` too (a
  `.toml` is read as a bundle otherwise), and it installs from the loopback
  registry because grim accepts no path source for `mcp`.
- `$REPO/target/release` comes first on `PATH`, so the renderer under test
  is the branch build, never an installed `grim`.
- The network is not denied at check time; see the Claude note below.
- Remove `$SB` and stop the registry container when the pass ends. The
  harness may refuse `rm -rf` on the scratchpad; `/usr/bin/find "$SB"
  -delete` works (the claude sandbox holds a ~230 MB binary).

### Per harness

| Harness | npm package | Config-root var = path | Telemetry opt-outs (besides `DO_NOT_TRACK=1`) | List command(s) |
|---|---|---|---|---|
| `claude` | `@anthropic-ai/claude-code` | `CLAUDE_CONFIG_DIR=$SB/home/.claude` | `DISABLE_TELEMETRY=1 DISABLE_ERROR_REPORTING=1 CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1` | `claude mcp list` |
| `codex` | `@openai/codex` | `CODEX_HOME=$SB/home/.codex` (create it first) | config keys only, passed as flags: `codex -c analytics.enabled=false -c feedback.enabled=false -c check_for_update_on_startup=false <verb>` | `codex mcp list`, `codex debug prompt-input` (skills) |
| `opencode` | `opencode-ai` | `OPENCODE_CONFIG_DIR=$SB/home/.opencode`, plus `TMPDIR=$SB/tmp` | `OPENCODE_DISABLE_AUTOUPDATE=1 OPENCODE_DISABLE_MODELS_FETCH=1` (no telemetry variable in `flag.ts` at v1.18.32) | `opencode agent list`, `opencode debug skill` (skills), `opencode debug config` (rules glob, MCP), `opencode mcp list` |
| `copilot` | `@github/copilot` | `COPILOT_HOME=$SB/home/.copilot` | `COPILOT_OFFLINE=true COPILOT_AUTO_UPDATE=false` (both in `copilot help environment` at 1.0.88; offline also drops the builtin `github-mcp-server`) | `copilot skill list`, `copilot instruction list`, `copilot mcp list`, `copilot mcp get <name>`; agents: `copilot -p x --agent no-such-agent` (see note) |

Known behavior, from `.agents/research/research_live_cli_sandbox_checks.md`:

- **claude** — MCP only: the CLI has no skill, rule or agent listing verb
  (`claude agents` manages background *sessions*, not agent files), so the
  verdict covers MCP and says so. `--ignore-scripts` leaves `bin/claude.exe`
  a stub that refuses to run; run the native binary the optional
  dependency ships instead,
  `$SB/npm/node_modules/@anthropic-ai/claude-code-<platform>/claude`
  (e.g. `linux-x64`) — no install script executes. In the 2026-09-27 run
  (`env -i`, no login, the opt-outs above) `claude mcp list` showed only
  the project entry; the five claude.ai connectors an earlier probe saw
  did not appear. If they do, check that no credential leaked into the
  sandbox before trusting the run. The signal is grim's entry shown as
  `⏸ Pending approval`, never dialled; `claude mcp get <name>` adds its
  scope (`Project config (shared via .mcp.json)`).
- **codex** — offline, no auth. Always warns that it will not create PATH
  aliases under `/tmp`; the scratchpad is under `/tmp`, so nesting does not
  help, and the warning is benign. Codex reads a project's
  `.codex/config.toml` only for a trusted project: before the check, write
  `[projects."$SB/project"]` with `trust_level = "trusted"` into
  `$SB/home/.codex/config.toml`, or `codex mcp list` shows nothing. Skills:
  `codex debug prompt-input` prints the model-visible context offline,
  including a `Skill roots` table and `Available skills`; pass when the
  fixture skill is listed. Agents have no headless verb, and a malformed
  role file shows no warning there either — `skip`. Do not use
  `codex doctor`: it dials the network.
- **opencode** — offline, no login for any of the four verbs.
  `--ignore-scripts` leaves `bin/opencode.exe` a stub that refuses to run;
  run the native binary instead,
  `$SB/npm/node_modules/opencode-<platform>/bin/opencode` (e.g.
  `linux-x64`). Without `TMPDIR` it creates `/tmp/opencode` outside the
  sandbox. The recipe works at both scopes: `grim --global install` from a
  directory outside the project, then run the verbs there. `opencode mcp
  list` dials every server: put `$REPO/target/release` on the check's
  `PATH` so grim's stdio entry reads `connected`; a fixture remote URL
  shows `failed`, which is expected. `debug config` shows `{env:VAR}`
  already resolved, so an unset variable reads empty there. OpenCode
  validates config strictly: one bad agent value (e.g. `mode: pilot`)
  makes every verb exit 1 with "Configuration is invalid", so a grim
  render bug would break the whole CLI, not only that agent. In zsh an
  unquoted `$cmd` holding `agent list` is not word-split and reaches
  OpenCode as a project path; spell the verbs out, or run the loop in
  `bash`.
- **copilot** — offline, no login for the list verbs; validates skill
  frontmatter, so parse errors are real findings. The npm loader needs
  `node`, which `env -i` hides; run the native binary the optional
  dependency ships, `$SB/npm/node_modules/@github/copilot-<platform>/copilot`
  (e.g. `linux-x64`). `copilot mcp list` shows a builtin
  `github-mcp-server` unless `COPILOT_OFFLINE=true`. The CLI reads
  workspace MCP from `.mcp.json`/`.github/mcp.json` only, so grim's
  project-scope `.vscode/mcp.json` entries never list — expected, not a
  render bug. Agents have no list verb: `copilot -p x --agent
  no-such-agent` exits with "No such agent: …, available: <names>" before
  any model call. To make the CLI start MCP servers (e.g. to prove `${VAR}`
  expansion with a stdio fixture that dumps its argv), add
  `COPILOT_PROVIDER_BASE_URL=http://127.0.0.1:9/v1 COPILOT_MODEL=gpt-4o`
  and run `copilot -p x --allow-all-tools`: servers start, the model call
  fails on the closed port, no credential exists. `mcp get` shows config
  as written, never expanded, so it cannot prove substitution.

### Recording the result

In the run artifact's `## Live CLI check`, one line per command:

```text
<command as run, env values elided> — pass | fail | skip — <reason>
```

`pass`: every grim-rendered fixture of that kind is listed without error.
`fail`: missing or rejected, with the error class (never raw output).
`skip`: install failed, login required, or no listing command for the kind.
Never paste raw CLI output; never log in to make a check pass.
