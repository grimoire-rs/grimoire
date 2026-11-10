---
paths:
  - src/install/vendor.rs
  - src/install/vendor_*.rs
  - src/install/prune.rs
  - src/install/claude_config.rs
  - src/install/opencode_config.rs
  - src/install/managed_config.rs
---

# Vendor Paths: Global-scope Roots

Global-scope client roots and the vendor environment overrides. Index: [subsystem-file-structure.md](./subsystem-file-structure.md).

Every `detect()` lives in `src/install/vendor_*.rs`, which never loads the
detection part, so a root or marker change here also updates
[subsystem-client-detection.md](./subsystem-client-detection.md#client-detection).
Each client's settings file is listed in
[subsystem-install-layout-settings.md](./subsystem-install-layout-settings.md#install-layout-settings).

### Global-scope paths {#global-scope-paths}

For a **global-scope** install (`--global`), grim writes into each
client's **native** user-level discovery directory rather than under
`$GRIM_HOME`, so the files are found without extra configuration:

| Client | Skills root | Rules path | Agents path |
|--------|-------------|------------|-------------|
| **Claude** | `~/.claude/skills/<name>/` | `~/.claude/rules/<name>.md` | `~/.claude/agents/<name>.md` |
| **OpenCode** | `$XDG_CONFIG_HOME/opencode/skills/<name>/` | `$GRIM_HOME/.opencode/rules/<name>.md` (absolute glob registered in global `opencode.json`) | `$XDG_CONFIG_HOME/opencode/agents/<name>.md` |
| **Copilot** | `~/.copilot/skills/<name>/` | `~/.copilot/instructions/<name>.instructions.md` (native; workspace-layout fallback + warn only when no root resolves) | `~/.copilot/agents/<name>.md` |
| **Codex** | `$HOME/.agents/skills/<name>/` (cross-vendor standard; independent of `$CODEX_HOME`) | **unsupported** — Codex has no path-scoped rule mechanism; grim skips it silently, writes no file | `$CODEX_HOME`\|`~/.codex/agents/<name>.toml` (TOML) |
| **Cursor** | `~/.cursor/skills/<name>/` | `~/.cursor/rules/<name>.mdc` | `~/.cursor/agents/<name>.md` |
| **Kiro** | `~/.kiro/skills/<name>/` | `~/.kiro/steering/<name>.md` | declined |
| **Junie** | `<junie_root>/skills/<name>/` (`$JUNIE_HOME` else `~/.junie`) | declined | `<junie_root>/agents/<name>.md` |
| **Gemini** | `$HOME/.agents/skills/<name>/` (shared pool) | declined | `<gemini_root>/agents/<name>.md` |
| **Zed** | `$HOME/.agents/skills/<name>/` (shared pool) | declined | declined |
| **Amp** | `$HOME/.agents/skills/<name>/` (shared pool) | declined | declined |
| **agents** | `$HOME/.agents/skills/<name>/` (shared pool — its only surface) | declined | declined |
| **Antigravity** | `~/.gemini/config/skills/<name>/` — **not** the pool at global scope | `~/.gemini/config/rules/<name>.md` | `~/.gemini/config/agents/<name>.md` |
| **Goose** | `$HOME/.agents/skills/<name>/` (shared pool, both scopes) | declined | `~/.goose/agents/<name>.md` |
| **Cline** | `~/.cline/skills/<name>/` | declined | declined |
| **Droid** | `~/.factory/skills/<name>/` (native alone; joins the pool beside a pool writer, `shared_skills` pins it) | declined | `~/.factory/droids/<name>.md` |
| **Warp** | `~/.warp/skills/<name>/` (native alone; joins the pool beside a pool writer, `shared_skills` pins it) | declined | declined |
| **OpenClaw** | `<openclaw_root>/skills/<name>/` (`$OPENCLAW_STATE_DIR`, else `$OPENCLAW_HOME/.openclaw`, else `~/.openclaw`) — **global-only client**, project scope writes nothing | declined | declined |
| **Kilo** | `~/.kilo/skills/<name>/` (native alone; joins the pool beside a pool writer, `shared_skills` pins it) | declined | `$XDG_CONFIG_HOME\|~/.config/kilo/agents/<name>.md` |
| **Qoder** | `<qoder_root>/skills/<name>/` | `<qoder_root>/rules/<name>.md` | `<qoder_root>/agents/<name>.md` |

`$XDG_CONFIG_HOME` falls back to `~/.config` when unset.

**Pool auto-join** (`adr_shared_pool_auto_join.md`, issue #262). A **verified
pool reader** (`POOL_CAPABLE_VENDORS` in `src/install/vendor.rs`: Cursor,
Copilot, OpenCode, Warp, Droid, Kilo, Cline) writes its *skills* to
`$HOME/.agents/skills/<name>/` instead of the root above whenever another
client in the effective set — this run's targets plus the clients the
artifact's record still holds — lands in the pool **unconditionally** (its
`path_for` is the pool path, or it is pinned `shared_skills = true`). Readers
never pull each other in, so OpenCode + Cursor alone stay native. A reader
that has switched its pool scan off (`Vendor::reads_pool`: OpenCode's and
Kilo's `*_DISABLE_EXTERNAL_SKILLS`, Copilot global with a diverging
`COPILOT_HOME`) stays native. `[options.vendors.<name>].shared_skills` is
tri-state: unset = auto, `true` = always pool, `false` = always native.
`true` is accepted only for a pool reader; on any other client it is exit
**65** at `grim config set` / **78** at load. The one seam is
`expected_outputs::destination`; `InstallTarget::path_for` stays the
pre-join path. A move in either direction is a layout move the existing
reapers handle (`reap_moved_outputs`; pool → native converges over two
`update` runs via `shared_by_surviving_sibling`).

**Vendor config-dir env vars that are NOT honored** (paths hardcode the
documented native root), each for its own reason:

- `CURSOR_CONFIG_DIR` — documented only as the location of the CLI's own
  `cli-config.json`, so grim reads it for that one file, the settings
  kind's global target ([Settings](./subsystem-install-layout-settings.md#install-layout-settings), the
  `cursor-cli` row), and for nothing else: skills, rules, agents, `mcp.json`
  and detection stay on `~/.cursor`. New with the kind, so no
  `relocated_vendor_roots` row.
- the `JUNIE_*_LOCATIONS` family — additive, not untested: it only adds
  search paths, so grim's defaults stay read. (`JUNIE_HOME`, which replaces
  `~/.junie` outright, **is** honored — see the override table.)
- `GEMINI_CONFIG_DIR` — genuinely does not exist upstream (only FR #2815).
  The variable that *does* exist is `GEMINI_CLI_HOME`, which grim honors —
  see the override table below.
- `$AMP_SETTINGS_FILE` — **contested, not disproven.** Amp's official
  manual documents only `--settings-file`, but a third-party string
  extraction from Amp's real shipped bundle lists the variable among its
  env vars. No source addresses its precedence relative to
  `.amp/settings.json`, or whether it names a file or a directory.
  Shipping behaviour on that evidence would be a coin flip, so behaviour
  is unchanged — the reason is *unaddressed precedence*, not
  *nonexistence*. Do not "correct" this back to "no such variable exists".

All are watchlisted for re-verification — see
`vendor-capability-watchlist.md`.

**XDG resolution is platform-dependent for Zed.** Zed and Amp both root
their global settings under `$XDG_CONFIG_HOME` (falling back to
`~/.config`), and so do Kilo (`kilo/`, every platform) and Warp on Linux
only (`warp-terminal/settings.toml`; `~/.warp` on macOS, no root elsewhere,
`vendor_warp::SettingsRootKind`), **but Zed consults XDG on Linux and FreeBSD only**:
upstream's `config_dir()` reads XDG on that branch alone, falls to a
literal `~/.config/zed` on macOS, and uses `%APPDATA%\Zed` on Windows.
`vendor_zed::zed_root_from` mirrors that split through an explicit
`ZedRootKind`, so every arm is testable on any host. **Amp is a separate
question and it is unresolved:** source-tier verification is unachievable
(compiled binary, stub npm package, no public repo), and the full manual
greps to zero XDG hits while documenting `~/.config/amp/` identically for
macOS and Linux. The shared `xdg_config_dir()` helper is therefore left
alone — making it platform-aware would move Amp's macOS resolution on
zero evidence.

**Vendor env overrides** (each client's own variable; the directory
variables are honored read-only, `OPENCODE_CONFIG` names a file grim reads
**and** rewrites; empty value = unset):

| Variable | Effect on global paths |
|----------|------------------------|
| `CLAUDE_CONFIG_DIR` | Replaces the entire `~/.claude` tree — Claude skills, rules, and agents root there. Also relocates the global MCP registration file to `$CLAUDE_CONFIG_DIR/.claude.json`. **Also read from Claude's own settings `env`** (`vendor_claude::config_dir_override`, the one resolver every Claude path uses): managed `managed-settings.json` then `managed-settings.d/*.json` (name order, last wins) > user `settings.json` in the shell-resolved root > shell > `~/.claude` — settings beat the shell because Claude writes each `env` entry over the inherited value (code.claude.com/docs/en/env-vars "Precedence"). Project/local settings are never read (Claude ≥ 2.1.251 ignores them for this key). Values are untrusted input: absolute, no `..`, nothing expanded; any failure drops that layer at `debug`; one leading BOM is stripped before the parse, as Claude does. Residual gaps: MDM/registry/server-managed policy (unreadable), `claude --settings` (flagSettings, per-session), a second hop (`<new root>/settings.json` setting it again — grim follows one), and a Windows rooted-without-drive `\x` (not `is_absolute`, ignored). Memoized per process (`OnceLock`) so one run never splits across roots. The managed dir is injected (`config_dir_from`) so tests never touch system paths |
| `COPILOT_HOME` | Replaces `~/.copilot` — Copilot skills and agents land under `$COPILOT_HOME/`. VS Code's embedded Copilot CLI honors it since VS Code 1.132.0 (microsoft/vscode#314917). **Caveat:** set to anything but `~/.copilot`, Copilot stops scanning `~/.agents/skills`, so `Vendor::reads_pool` keeps Copilot's skills native there (no auto-join), and only an explicit `shared_skills = true` still writes skills it never reads (warned; watchlist) |
| `OPENCODE_CONFIG_DIR` | OpenCode's additive scan dir — preferred over the XDG default for skills and agents when set |
| `OPENCODE_CONFIG` | Config **file** path only (global `opencode.json` edit target); no effect on skill/agent paths |
| `CODEX_HOME` | Replaces `~/.codex` — Codex **agents** root **and** the MCP `config.toml` there. Does **not** relocate Codex skills (those follow the `$HOME/.agents/skills` cross-vendor standard) |
| `KIRO_HOME` | Replaces `~/.kiro` **outright, no `.kiro` segment appended** — the `CODEX_HOME` shape. Kiro skills, `steering/` rules, `settings/mcp.json` and the settings kind's `settings/cli.json` all follow it. grim follows the Kiro **CLI**; the Kiro **IDE** still hardcodes `~/.kiro` and ignores the variable (kirodotdev/Kiro#9148) — that is an upstream fact, not a limit on what grim honors. A user who sets it *and* uses the IDE gets output where the CLI reads it, not the IDE |
| `QODER_CONFIG_DIR` | Replaces `~/.qoder` **outright** — the `KIRO_HOME` shape. Qoder skills, rules, agents, and `settings.json` all follow it. Always honored since Qoder support landed, so **no** `relocated_vendor_roots` row |
| `JUNIE_HOME` | Replaces `~/.junie` **outright** — the `KIRO_HOME` shape. Junie skills, agents, and `mcp/mcp.json` follow it (`vendor_junie::junie_root`, the one resolver) |
| `OPENCLAW_STATE_DIR`, `OPENCLAW_HOME` | `OPENCLAW_STATE_DIR` names the state root itself and wins; else `OPENCLAW_HOME` replaces the home directory, so the root is `$OPENCLAW_HOME/.openclaw` (the `GEMINI_CLI_HOME` shape). Skills land in `<root>/skills`. A leading `~` expands to the real home (upstream does the same); any other relative value is ignored at `debug`, never CWD-resolved |
| `GEMINI_CLI_HOME` | **The opposite shape.** It replaces Node's `os.homedir()`, and Gemini then joins `.gemini` onto it — so the root is `$GEMINI_CLI_HOME/.gemini`, with the segment still appended. Relocates Gemini's `agents/` and `settings.json`. Deliberately does **not** relocate the shared `.agents/skills` pool, which stays keyed on the real `$HOME` (see below) |

**The settings kind fences these roots; no other kind does.** A global settings
file whose root a variable makes relative, or places inside the repository the
command runs in, is declined (`settings-root-relative` /
`settings-root-in-workspace`, `settings_phase::root_refusal`). Skills, rules,
agents and MCP entries still follow the variable there.

**The two shapes are opposites — do not conflate them.** `KIRO_HOME` and
`CODEX_HOME` *replace* their directory; `GEMINI_CLI_HOME` replaces the
*home directory* and the vendor segment is still appended.

**The shared pool never follows a vendor override.** `AgentsSkills` is
keyed on the real `$HOME` for every member. Upstream Gemini *does* derive
its own pool from the overridden homedir, so this is a knowing divergence:
grim writes **one** physical pool tree, deduped to a single destination and
released by `prune.rs`'s refcount guard, and a per-vendor pool root would
fork that tree and break the one-path/N-outputs shape both the guard and
the installer's dest-dedup rest on. **Residual gap, open:** a user who sets
`GEMINI_CLI_HOME` gets pool skills at `$HOME/.agents/skills` while Gemini
reads `$GEMINI_CLI_HOME/.agents/skills`. Watchlisted.

**A newly honored override is a layout move.** `KIRO_HOME`,
`GEMINI_CLI_HOME`, `JUNIE_HOME`, `OPENCLAW_STATE_DIR`/`OPENCLAW_HOME`, the
settings-sourced `CLAUDE_CONFIG_DIR`, and the Zed macOS XDG correction all relocate a root
for users who already set the variable, so `installer.rs::reap_relocated_roots`
sweeps the pre-override root on install, update, and uninstall.
`relocated_vendor_roots` is the closed table of roots that actually moved —
**never add a row for a variable grim always honored** (the shell
`CLAUDE_CONFIG_DIR`, `COPILOT_HOME`, `CODEX_HOME`, `OPENCODE_CONFIG_DIR`): their pre-override
location is the *default* root, which grim itself very likely populated in
an earlier no-override session, and a row would delete those copies. The
Claude rows are minted only when the settings-derived value differs from the
shell-only one, and move **both** Claude roots (`claude-root` and
`claude-user-dir`, the latter via the `CLAUDE_USER_DIR_ROW` pseudo-row).

**Fallback**: env override → native default (`$HOME`-derived) → workspace
layout under `$GRIM_HOME` for the affected client.
