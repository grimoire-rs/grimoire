---
paths:
  - src/**
---

# File Structure Subsystem

How Grimoire lays out its on-disk data: where downloaded artifacts, the
local index, and install links live under the data root.

## Design Rationale

- **Single data root.** All Grimoire state lives under one directory
  (default `~/.grimoire`, overridable via `GRIM_HOME`). Keeping everything
  under one root makes atomic rename / hardlink operations possible because
  source and destination stay on one filesystem.
- **Content-addressed storage.** Downloaded artifacts are addressed by
  content hash so identical content is stored once and is immutable.
- **Mutable namespace on top.** Human-facing names (tags, "installed"
  links) are a thin mutable layer pointing at immutable content.
- **Cross-device safety.** Operations that rely on same-filesystem
  atomic rename must validate the data root sits on a single volume;
  cross-device hardlink/rename fails and must be handled explicitly.

## Install Layout (client targets)

### Skills

A **skill** materializes as a directory tree under the client's `skills/`
dir. Every file in the tree is copied verbatim, **except** `SKILL.md` when
it carries tool-namespaced metadata keys (e.g. `claude.user-invocable` in
the `metadata` map). In that case `SKILL.md` is **rendered per client**:
known `<client>.<field>` keys are lifted to native typed top-level
frontmatter, foreign-namespace keys are dropped, and the written file is
marked `generated: true`. A plain skill with no tool-namespaced keys takes
the fast path and installs byte-identical. See `arch-principles.md` ADR
index → `adr_tool_namespaced_metadata_rendering.md`.

### Rules

A **rule** materializes as the index `<name>.md` under `rules/`, and —
when the artifact carries an optional sibling support directory — that
directory installs **beside** the index as `rules/<name>/…` so the index's
relative links resolve. The two on-disk roots (index file + sibling dir)
are one footprint: the integrity hash folds both, and uninstall removes
both. See `arch-principles.md` ADR index → `adr_multifile_rules.md`.

**Most clients decline rules** (`Vendor::kind_support(Rule) == Declined`) —
Codex, Gemini, Zed, Amp, Antigravity, the six skills-only clients, and the
generic `agents` target all lack an ownable path-scoped instruction surface
(AGENTS.md / GEMINI.md hierarchies, or a UI-managed surface with no on-disk
path). The installer warns + skips, writes no file, and records no output.
**Junie is Degraded, not Declined** — `.junie/rules/*.md` is a real
per-file directory grim can own; what it lacks is a per-file activation
key, so `paths` is dropped with a warning. **`docs/src/clients.md` is the
enforced matrix** — a parity test reads it at test time, so trust it over
any prose list here. Background: `arch-principles.md` ADR index →
`adr_codex_vendor.md`.

**A kind can also be declined at one scope only** — `Vendor::kind_surface(kind,
scope) -> bool`, defaulted `true`, consulted from `client_supports_kind`
after `kind_support` has already passed. Two exceptions exist today, and
the complete set is pinned as `SCOPE_GAPS` in `vendor.rs`'s tests:
Junie rules at **global** scope (`.junie/rules/` exists; `~/.junie/rules/`
does not) and OpenClaw skills at **project** scope (its "project" path is a
fixed daemon home that does not track the repo). Add a scope gap here,
never by changing `kind_support`'s signature.

Per-client rule transforms:

- **Claude**: `paths:` is native Claude rule frontmatter. A plain rule
  carrying no tool-namespaced metadata keys installs verbatim, marked
  `generated: false` (fast path). A rule that carries any
  `<vendor>.<field>` entry inside its `metadata` map is re-rendered:
  own-namespace Claude keys lift per registry (empty today — unknown ones
  warn + drop), foreign vendor keys drop silently, plain keys survive.
  Written `generated: true`; if cleaned frontmatter is empty, the block
  is omitted entirely.
- **OpenCode**: frontmatter is stripped; the file written is a provenance
  comment followed by the rule body. Marked `generated: true`. Loading is
  wired through a managed glob entry in `opencode.json` (or `opencode.jsonc`
  when present). grim adds the entry when the first OpenCode rule installs
  and removes it when the last one uninstalls; the target file is
  `.opencode/rules/<name>.md`.
- **Copilot**: written to `.github/instructions/<name>.instructions.md`.
  Frontmatter maps `paths` → `applyTo` (comma-joined into a single string)
  and the optional `copilot.exclude-agent` key (authored in rule `metadata`)
  → `excludeAgent` (enum: `code-review` or `cloud-agent`). A rule with
  neither produces no frontmatter block at all. Marked `generated: true`.
- **Cursor**: written to `.cursor/rules/<name>.mdc` (native `.mdc`,
  `~/.cursor/rules/` at global). Frontmatter always present: `paths`
  comma-joins into the single `globs` STRING plus `alwaysApply: false`;
  unscoped → no `globs` and `alwaysApply: true`. Marked `generated: true`.
- **Kiro**: written to `.kiro/steering/<name>.md` (`~/.kiro/steering/` at
  global). Global steering ships inert (no per-file `fileMatch` scoping yet
  — watchlisted #9176) + warn. Marked `generated: true`.

Support directory files are copied verbatim for every rule-supporting
client (Claude, OpenCode, Copilot, Cursor, Kiro). Only the index is ever
transformed.

### MCP servers {#install-layout-mcp}

An **mcp** artifact never materializes a file. Install registers a
vendor-native entry in each client's own MCP config file via a
span-preserving splice (every byte outside the managed member — key
order, formatting, comments — survives): JSON/JSONC for every client but
Codex (`src/install/json_splice.rs`), TOML for Codex
(`src/install/toml_splice.rs`, built on `toml_edit`). Uninstall removes
only that entry, never the file, through the same format-specific
splice engine (dispatched on `Vendor::mcp_config_format`). Integrity is
judged **semantically** on the entry value (canonical sorted-key JSON
hash — even for the TOML target, whose entry is converted to JSON before
hashing), so reformatting the config is not a modification. Per-client
config files:

| Client | Project | Global |
|--------|---------|--------|
| **Claude** | `<workspace>/.mcp.json` (`mcpServers`) | `~/.claude.json` — `$CLAUDE_CONFIG_DIR/.claude.json` when set (`mcpServers`) |
| **OpenCode** | `<workspace>/opencode.json`/`.jsonc` (`mcp`) | `$OPENCODE_CONFIG` else XDG `opencode.json` (`mcp`) |
| **Copilot** | `<workspace>/.vscode/mcp.json` (`servers`) | `$COPILOT_HOME`\|`~/.copilot`/`mcp-config.json` (`mcpServers`); env-ref descriptors are skipped (no substitution support) |
| **Codex** | `<workspace>/.codex/config.toml` (`mcp_servers`); only honored by Codex for **trusted** projects — grim writes it regardless, an untrusted project simply won't have it read | `$CODEX_HOME`\|`~/.codex`/`config.toml` (`mcp_servers`); HTTP/SSE headers map to `http_headers`/`env_http_headers`/`bearer_token_env_var`; a header embedding an env ref in text is unrepresentable → descriptor skipped |
| **Cursor** | `<workspace>/.cursor/mcp.json` (`mcpServers`); stdio needs `type: "stdio"`, env refs `${env:VAR}` | `~/.cursor/mcp.json` (`mcpServers`) |
| **Kiro** | `<workspace>/.kiro/settings/mcp.json` (`mcpServers`); `${VAR}` env refs native | `~/.kiro/settings/mcp.json` (`mcpServers`) |
| **Junie** | `<workspace>/.junie/mcp/mcp.json` (`mcpServers`); env-ref descriptors skipped (interpolation undocumented) | `~/.junie/mcp/mcp.json` (`mcpServers`) |
| **Gemini** | `<workspace>/.gemini/settings.json` (`mcpServers`); sse → `url`, http → `httpUrl`, `${VAR}` native | `~/.gemini/settings.json` (`mcpServers`) |
| **Zed** | `<workspace>/.zed/settings.json` (`context_servers`, flat shape); env-ref descriptors skipped (no upstream support) | `$XDG_CONFIG_HOME`\|`~/.config/zed` (unix) or `%APPDATA%\Zed` (Windows) `/settings.json` (`context_servers`, JSONC) |
| **Amp** | `<workspace>/.amp/settings.json` (`amp.mcpServers`, literal dotted key); `${VAR}` refs | `$XDG_CONFIG_HOME`\|`~/.config/amp`/`settings.json` (`amp.mcpServers`) |

Every non-Claude client declines the `ws` transport and the structured
`oauth` block (skip + warn) — see `docs/src/clients.md` "Known gaps".

### Agents {#install-layout-agents}

An **agent** materializes as a single file in the client's agents
directory (no support directory). For Claude/OpenCode/Copilot/Cursor/Gemini
it is a Markdown file (Claude installs a plain agent verbatim; the others
project the frontmatter, each lifting its own `<vendor>.*` field registry —
e.g. `cursor.readonly`, `gemini.temperature`). For **Codex** it is a
**TOML** file (`<name>.toml`) — Codex is the only TOML-emitting vendor: the
canonical `name`/`description` plus the agent body (as
`developer_instructions`) and an optional `model` are serialized to TOML;
the `tools` field has no Codex equivalent and is dropped with a warning.
For **Antigravity** it is a Markdown file too, with `tools` emitted as a
YAML sequence (upstream types it `string[]`) and nothing lifted — the
`antigravity.*` registry is empty. **Every other client declines agents**
(`kind_support == Declined`): CLI/IDE schema collision (Kiro), EAP-only
(Junie), ACP/runtime-only (Zed, Amp, Goose, OpenClaw), or no installable
format at all (Cline, Droid, Warp, Kilo, and the generic `agents` target) —
installer warns + skips, records no output.

Per-client agent paths:

| Client | Global agent path |
|--------|-------------------|
| **Claude** | `<claude_root>/agents/<name>.md` |
| **Copilot** | `<copilot_root>/agents/<name>.md` |
| **OpenCode** | `<opencode_root>/agents/<name>.md` |
| **Codex** | `<codex_root>/agents/<name>.toml` |
| **Cursor** | `~/.cursor/agents/<name>.md` (project `.cursor/agents/`) |
| **Gemini** | `<gemini_root>/agents/<name>.md` (project `.gemini/agents/`) |
| **Antigravity** | `~/.gemini/config/agents/<name>.md` (project `.agents/agents/`) |
| **everyone else** | declined — no agent surface |

`opencode_root` is the parent of the OpenCode skills directory (i.e. the
directory one level above the `skills/` subdir resolved from
`$OPENCODE_CONFIG_DIR` or the XDG default). `claude_root`, `copilot_root`,
`codex_root` and `gemini_root` are the vendor roots in the
[`VendorRoot` table](#path-anchor-set) — `$CODEX_HOME` else `~/.codex`,
and `$GEMINI_CLI_HOME/.gemini` else `~/.gemini`.

### Global-scope paths {#global-scope-paths}

For a **global-scope** install (`--global`), grim writes into each
client's **native** user-level discovery directory rather than under
`$GRIM_HOME`, so the files are found without extra configuration:

| Client | Skills root | Rules path | Agents path |
|--------|-------------|------------|-------------|
| **Claude** | `~/.claude/skills/<name>/` | `~/.claude/rules/<name>.md` | `~/.claude/agents/<name>.md` |
| **OpenCode** | `$XDG_CONFIG_HOME/opencode/skills/<name>/` | `$GRIM_HOME/.opencode/rules/<name>.md` (absolute glob registered in global `opencode.json`) | `$XDG_CONFIG_HOME/opencode/agents/<name>.md` |
| **Copilot** | `~/.copilot/skills/<name>/` | `~/.copilot/instructions/<name>.instructions.md` (native; workspace-layout fallback + warn only when no root resolves) | `~/.copilot/agents/<name>.md` |
| **Codex** | `$HOME/.agents/skills/<name>/` (cross-vendor standard; independent of `$CODEX_HOME`) | **unsupported** — Codex has no path-scoped rule mechanism; grim warns + skips, writes no file | `$CODEX_HOME`\|`~/.codex/agents/<name>.toml` (TOML) |
| **Cursor** | `~/.cursor/skills/<name>/` | `~/.cursor/rules/<name>.mdc` | `~/.cursor/agents/<name>.md` |
| **Kiro** | `~/.kiro/skills/<name>/` | `~/.kiro/steering/<name>.md` | declined |
| **Junie** | `~/.junie/skills/<name>/` | declined | declined |
| **Gemini** | `$HOME/.agents/skills/<name>/` (shared pool) | declined | `<gemini_root>/agents/<name>.md` |
| **Zed** | `$HOME/.agents/skills/<name>/` (shared pool) | declined | declined |
| **Amp** | `$HOME/.agents/skills/<name>/` (shared pool) | declined | declined |
| **agents** | `$HOME/.agents/skills/<name>/` (shared pool — its only surface) | declined | declined |
| **Antigravity** | `~/.gemini/config/skills/<name>/` — **not** the pool at global scope | declined | `~/.gemini/config/agents/<name>.md` |
| **Goose** | `$HOME/.agents/skills/<name>/` (shared pool, both scopes) | declined | declined |
| **Cline** | `~/.cline/skills/<name>/` | declined | declined |
| **Droid** | `~/.factory/skills/<name>/` | declined | declined |
| **Warp** | `~/.warp/skills/<name>/` (native by default; the pool only via `shared_skills`) | declined | declined |
| **OpenClaw** | `~/.openclaw/skills/<name>/` — **global-only client**, project scope writes nothing | declined | declined |
| **Kilo** | `~/.kilo/skills/<name>/` | declined | declined |

`$XDG_CONFIG_HOME` falls back to `~/.config` when unset. A client whose
`[options.vendors.<name>].shared_skills` is set writes its skills to
`$HOME/.agents/skills/<name>/` instead of the root above; the opt-in is
accepted only for a **verified pool reader** (`POOL_CAPABLE_VENDORS` in
`src/install/vendor.rs`), and setting it on any other client is exit **65**
at `grim config set` / **78** at load.

**Vendor config-dir env vars that are NOT honored** (paths hardcode the
documented native root), each for its own reason:

- `CURSOR_CONFIG_DIR` — possibly CLI-only; never verified against the IDE.
- the `JUNIE_*_LOCATIONS` family — per-kind override family, untested.
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
- `$OPENCLAW_HOME` — referenced in OpenClaw material but never defined on
  any page fetched.

All are watchlisted for re-verification — see
`vendor-capability-watchlist.md`.

**XDG resolution is platform-dependent for Zed.** Zed and Amp both root
their global settings under `$XDG_CONFIG_HOME` (falling back to
`~/.config`), **but Zed consults XDG on Linux and FreeBSD only**:
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
| `CLAUDE_CONFIG_DIR` | Replaces the entire `~/.claude` tree — Claude skills, rules, and agents root there. Also relocates the global MCP registration file to `$CLAUDE_CONFIG_DIR/.claude.json` |
| `COPILOT_HOME` | Replaces `~/.copilot` — Copilot skills and agents land under `$COPILOT_HOME/`. **Caveat:** correct for the standalone Copilot CLI, but VS Code's *embedded* Copilot CLI ignores the variable (microsoft/vscode#314806, open), so "Copilot" is not uniform here |
| `OPENCODE_CONFIG_DIR` | OpenCode's additive scan dir — preferred over the XDG default for skills and agents when set |
| `OPENCODE_CONFIG` | Config **file** path only (global `opencode.json` edit target); no effect on skill/agent paths |
| `CODEX_HOME` | Replaces `~/.codex` — Codex **agents** root **and** the MCP `config.toml` there. Does **not** relocate Codex skills (those follow the `$HOME/.agents/skills` cross-vendor standard) |
| `KIRO_HOME` | Replaces `~/.kiro` **outright, no `.kiro` segment appended** — the `CODEX_HOME` shape. Kiro skills, `steering/` rules, and `settings/mcp.json` all follow it. grim follows the Kiro **CLI**; the Kiro **IDE** still hardcodes `~/.kiro` and ignores the variable (kirodotdev/Kiro#9148) — that is an upstream fact, not a limit on what grim honors. A user who sets it *and* uses the IDE gets output where the CLI reads it, not the IDE |
| `GEMINI_CLI_HOME` | **The opposite shape.** It replaces Node's `os.homedir()`, and Gemini then joins `.gemini` onto it — so the root is `$GEMINI_CLI_HOME/.gemini`, with the segment still appended. Relocates Gemini's `agents/` and `settings.json`. Deliberately does **not** relocate the shared `.agents/skills` pool, which stays keyed on the real `$HOME` (see below) |

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
`GEMINI_CLI_HOME`, and the Zed macOS XDG correction all relocate a root
for users who already set the variable, so `installer.rs::reap_relocated_roots`
sweeps the pre-override root on install, update, and uninstall.
`relocated_vendor_roots` is the closed table of roots that actually moved —
**never add a row for a variable grim always honored** (`CLAUDE_CONFIG_DIR`,
`COPILOT_HOME`, `CODEX_HOME`, `OPENCODE_CONFIG_DIR`): their pre-override
location is the *default* root, which grim itself very likely populated in
an earlier no-override session, and a row would delete those copies.

**Fallback**: env override → native default (`$HOME`-derived) → workspace
layout under `$GRIM_HOME` for the affected client.

## Install State {#install-state}

Grimoire records what it installed, where, and at what content hash. The
record location differs by scope.

### Project state {#install-state-project}

Project install state lives at `<workspace>/.grimoire/state.json` — inside
a `.grimoire/` directory co-located with `grimoire.toml`. The workspace
directory is the key; there is no content hash of the config path in the
filename. Each project has exactly one state file at this fixed location,
so two projects sharing a common ancestor (or a shared `GRIM_HOME` volume)
cannot collide.

**Self-managed `.gitignore`**: the first time grim creates the `.grimoire/`
directory it writes `.grimoire/.gitignore` with contents `*` (if absent —
never overwrites a user-edited one). The consumer's root `.gitignore` is
never touched. This mirrors the convention used by [uv] (`.venv/.gitignore`)
and [pixi] (`.pixi/.gitignore`): the tool owns its dot-dir and excludes
its own contents from version control.

**Devcontainer named-volume caution**: if a devcontainer mounts a named
Docker volume at `<workspace>/.grimoire`, that volume shadows the
bind-mounted workspace state. A `grim install` inside the container writes
to the named volume, which is invisible to the host and to other containers
that bind-mount the same workspace directory. Use a bind-mount (not a
named volume) at `<workspace>/.grimoire` if you need state to be shared.

**Non-UTF-8 path components**: any path component that is not valid UTF-8
is rejected at store time with `UnknownAnchor`. All anchor roots must be
representable as UTF-8.

**Reap window**: between the read-only `load()` in a first post-upgrade
`status` call and the mutating `save()` in the next mutating command, the
old legacy state file (`$GRIM_HOME/state/projects/<sha>.json`) still
exists. A concurrent observer looking at the legacy path during this window
may see the old file even though the in-memory view is already migrated.
This is transient; the next mutating command reaps the legacy file.

**Nesting constraint**: `GRIM_HOME` must not be nested inside a workspace
directory. If it is, `from_target` may match `GrimHome` as the anchor for
a path that should classify as `Workspace`, producing an incorrect record.

### Global state {#install-state-global}

Global install state lives at `$GRIM_HOME/state/global.json`. This
location is unchanged from previous versions.

**Residual risk under a shared GRIM_HOME**: when two machines or containers
share the same `GRIM_HOME` volume, both read and write the same
`global.json`. Anchoring makes the stored *paths* portable (each machine
resolves anchor roots from its own environment), but the *record set* in
the file is shared. Concurrent or serial `grim install --global` calls
from different machines are last-writer-wins on the record set — the same
class of collision that project state now avoids. **v1 stance: single
writer at a time.** Per-host segmentation (keyed by a machine identity
such as `devcontainerId`) is a tracked follow-up, not part of this change.
`atomic_write` prevents partial-file corruption; only record-set
last-writer-wins is a residual risk.

### PathAnchor set {#path-anchor-set}

Stored paths are anchor-relative rather than absolute, so a state file
written on one machine resolves correctly on another (portable `$HOME`,
devcontainer portability). Every stored path carries an `anchor` tag and a
`relative` string (forward-slash UTF-8, Normal components only — no `.`,
`..`, leading `/`, or drive prefix).

The anchor set is **six fixed variants plus one parameterized
`VendorRoot(&'static str)`**. A vendor root serializes as
`<vendor-name>-root` — `cursor-root`, `kiro-root`, `antigravity-root` —
with **no `vendor:` prefix**. That is the same on-disk vocabulary every
shipped `state.json` already carries, because `Vendor::name()` equals each
former variant's tag prefix; collapsing the per-vendor variants into one
parameterized variant was a pure internal refactor with **zero on-disk
change**.

Fixed anchors:

| Anchor | Serde tag | Resolved root |
|--------|-----------|---------------|
| `Workspace` | `workspace` | The workspace directory passed to the CLI |
| `GrimHome` | `grim-home` | `$GRIM_HOME`. Also the universal fallback: appended to every pair's candidate list |
| `ClaudeUserDir` | `claude-user-dir` | `$CLAUDE_CONFIG_DIR` else `$HOME` — the dir holding Claude's user config file `.claude.json`. A *second*, differently-shaped root for one vendor, so it cannot be a `VendorRoot` row: with the override set the file lives *inside* it, without it the file is a *sibling* of `~/.claude` |
| `AgentsSkills` | `agents-skills` | `$HOME/.agents/skills` — the cross-vendor shared skills pool. Belongs to no single vendor, the root already ends in `/skills` (so `relative` is the bare skill name), and it is **never** relocated by a vendor `*_HOME` |
| `OpenCodeSkills` | `open-code-skills` | `$OPENCODE_CONFIG_DIR/skills` else `$XDG_CONFIG_HOME/opencode/skills` — a skills dir one level *below* the config root |
| `OpenCodeRoot` | `open-code-root` | Parent of the `OpenCodeSkills` root. Derived at lookup time, so there is no stored root and **`opencode` must never get a `VENDOR_ROOTS` row** — that would be two spellings of one location in `state.json`, which the reaper and the prune refcount treat as distinct outputs |

Note the two OpenCode tags: `rename_all = "kebab-case"` split the variant
names on their internal capital, so the on-disk tags are `open-code-*`, not
`opencode-*`. `Display` was aligned onto serde (error text only; the tags
never moved).

`VendorRoot` rows live in `VENDOR_ROOTS` (`src/install/path_anchor.rs`),
one per vendor whose global root is a plain `Option<PathBuf>`. Each row is
`(name, fn(EnvLookup, Option<PathBuf>) -> Option<PathBuf>)` — a pure
function of injected env and home, so the wiring is hermetically testable;
**never call `env_dir`/`home_dir` inside a row.** The resolvers run exactly
once, inside `AnchorRoots::resolve`, which is the single place ambient env
is read.

| Vendor row | Tag | Resolved root |
|---|---|---|
| `claude` | `claude-root` | `$CLAUDE_CONFIG_DIR` else `~/.claude` |
| `copilot` | `copilot-root` | `$COPILOT_HOME` else `~/.copilot` |
| `codex` | `codex-root` | `$CODEX_HOME` else `~/.codex` (hosts Codex `agents/` **and** the MCP `config.toml`) |
| `cursor` | `cursor-root` | `~/.cursor` (`CURSOR_CONFIG_DIR` not honored; hosts skills, `.mdc` rules, agents, `mcp.json`) |
| `kiro` | `kiro-root` | `$KIRO_HOME` else `~/.kiro` (hosts skills, `steering/` rules, `settings/mcp.json`) |
| `junie` | `junie-root` | `~/.junie` (hosts skills, `mcp/mcp.json`) |
| `gemini` | `gemini-root` | `$GEMINI_CLI_HOME/.gemini` else `~/.gemini` — segment appended either way (hosts `agents/`, `settings.json` MCP; skills use `AgentsSkills`) |
| `zed` | `zed-root` | Linux/FreeBSD: `$XDG_CONFIG_HOME` else `~/.config`, then `/zed`; macOS: literal `~/.config/zed`; Windows: `%APPDATA%\Zed` (hosts `settings.json` MCP; skills use `AgentsSkills`) |
| `amp` | `amp-root` | `$XDG_CONFIG_HOME` else `~/.config`, then `/amp` (hosts `settings.json` MCP; skills use `AgentsSkills`) |
| `antigravity` | `antigravity-root` | `~/.gemini/config` — nested under, and distinct from, the `gemini` row. Each client's candidate set holds only its own root, so the nesting never cross-classifies |
| `cline` | `cline-root` | `~/.cline` |
| `droid` | `droid-root` | `~/.factory` — **the tag follows the CLIENT name, the directory follows the vendor's.** Both are frozen and they differ on purpose |
| `warp` | `warp-root` | `~/.warp` (identical on macOS, Linux and Windows, deliberately so upstream) |
| `openclaw` | `openclaw-root` | `~/.openclaw` |
| `kilo` | `kilo-root` | `~/.kilo` (the legacy `.kilocode` is read for detection only, never written) |

`goose` has **no row and no tag**: it renders into the shared pool at both
scopes, so everything it writes anchors at `AgentsSkills` and a vendor root
would never be reachable. The vendor-neutral `agents` client likewise has
no row.

All roots are resolved once at scope-resolution time and passed as an
`AnchorRoots` struct — now a `BTreeMap<&'static str, PathBuf>` rather than
per-vendor fields — so every downstream operation is a pure table-lookup
with no ambient environment access. An absent vendor is an absent key.

**A row's name is an on-disk contract.** Rows may be appended; a name may
never be changed or removed (Principle 9). Adding a vendor costs one row
plus its `(client, kind)` arms in `candidate_anchors`, an entry in
`SHIPPED_ANCHOR_TAGS`, and a row in `hermetic_vendor_roots` — no enum
variant, no `AnchorRoots` field, no fixture churn. Never add a row whose
`<name>-root` collides with a fixed tag: the fixed arms of
`from_serde_tag` are matched first, so a colliding row would be written
under its own name and silently read back as the *other* anchor.

### Anchor root/remainder table {#anchor-remainder-table}

Authoritative mapping from `(scope, client, kind)` to `(anchor, stored relative)`:

Anchors are named below by their **serde tag** — what actually lands in
`state.json`.

| Scope · client · kind | Anchor | Stored `relative` |
|---|---|---|
| project · any · any | `workspace` | `.claude/…`, `.opencode/…`, `.github/…`, `.agents/…`, `.codex/…` (full sub-path from workspace) |
| global · claude · skill | `claude-root` | `skills/<name>` |
| global · claude · rule | `claude-root` | `rules/<name>.md` |
| global · claude · agent | `claude-root` | `agents/<name>.md` |
| global · copilot · skill | `copilot-root` | `skills/<name>` |
| global · copilot · agent | `copilot-root` | `agents/<name>.md` |
| global · opencode · skill | `open-code-skills` | `<name>` (root already ends `/skills`) |
| global · opencode · agent | `open-code-root` | `agents/<name>.md` |
| global · opencode · rule | `grim-home` | `.opencode/rules/<name>.md` |
| global · copilot · rule | `copilot-root` | `instructions/<name>.instructions.md` (the `grim-home` fallback classifies pre-move records for the layout-migration reaper) |
| global · codex · skill | `agents-skills` | `<name>` (root already ends `/skills`) |
| global · codex · agent | `codex-root` | `agents/<name>.toml` |
| · codex · rule | — | **not classified** — declined at the `kind_support` gate before anchoring; no output recorded |
| project · any · mcp | `workspace` | client-specific config path from the [MCP table](#install-layout-mcp) (`.mcp.json`, `.cursor/mcp.json`, `.kiro/settings/mcp.json`, …); entry-typed output — `entry` carries the managed member's JSON pointer |
| global · claude · mcp | `claude-user-dir` | `.claude.json` |
| global · opencode · mcp | `open-code-root` | `opencode.json` |
| global · copilot · mcp | `copilot-root` | `mcp-config.json` |
| global · codex · mcp | `codex-root` | `config.toml` (same root as the agent anchor — TOML splice, see [install-layout-mcp](#install-layout-mcp)) |

**Every other vendor** follows the same `(scope, client, kind)` →
`(anchor, relative)` shape, with `<name>-root` as its global anchor and the
`relative` remainder taken verbatim from the Install Layout and
[Global-scope](#global-scope-paths) tables above — e.g. global · cursor ·
rule → `cursor-root` + `rules/<name>.mdc`; global · kiro · mcp →
`kiro-root` + `settings/mcp.json`. Three exceptions worth stating:

- **Global pool skills anchor at `agents-skills`, never at a vendor
  root** — Codex, Gemini, Zed, Amp, Goose, the generic `agents` client,
  and any client whose `[options.vendors.<name>].shared_skills` opt-in is
  active. Goose has no vendor root at all. (At *project* scope every
  triple anchors at `workspace`, pool or not; the pool shows up only in
  the `relative` remainder, `.agents/skills/<name>`.)
- **Antigravity is a partial pool member.** Its *project* skills pool into
  `.agents/skills`; its *global* skills anchor at
  `antigravity-root` (`~/.gemini/config/skills/<name>`). Nothing in the
  installer or prune assumes scope-uniform pool membership — both key on
  resolved destination paths, not on a roster. **Pool *capability* is
  scope-blind**, though: a partial member must not join
  `POOL_CAPABLE_VENDORS`, or an opt-in would write global skills where the
  client never scans, and nothing would fail.
- **The opt-in classifies both layouts for one triple.**
  `candidate_anchors` cannot see the config, so a pool-capable client's
  skill triple returns `[native root, agents-skills, grim-home]` — native
  first, so a root tie keeps the client's own anchor rather than silently
  re-anchoring a user who never opted in.

Declined `(client, kind)` pairs are never classified — skipped at the
`kind_support` gate before anchoring. A pair declined only *at one scope*
(`Vendor::kind_surface(kind, scope) == false`, today Junie rules at global
and OpenClaw skills at project) is likewise never classified at that scope:
the installer warns, skips, and records zero outputs.

### Path containment guard {#path-containment-guard}

`AnchoredPath::resolve` enforces containment through two layers before any
filesystem operation runs on the joined path:

**Layer 1 (always, works for absent paths)**: every component of `relative`
must be `Normal`. Any `ParentDir` (`..`), `CurDir` (`.`), `RootDir`, or
`Prefix` component causes an immediate `TraversalAttempt` error without
touching the filesystem.

**Layer 2 (only when the candidate path exists)**: `dunce::canonicalize`
resolves both the candidate path and the anchor root, then asserts
`candidate.starts_with(anchor_root)` at the component boundary. A symlink
inside the anchor pointing outside it yields `EscapedAnchor`.

No consumer joins anchor + relative manually. Every filesystem operation
(read, hash, delete) receives the result of `resolve()`, never the raw
`relative` string.

See `quality-security.md` for the path-traversal and symlink-escape guard
principles that this two-layer pattern implements.

## Client Detection (default install targets) {#client-detection}

When neither `--client` nor the config `[options].clients` selects a
client, `install` / `update` / TUI target **all detected clients**. A
client is detected when its vendor directory / config marker is present
for the active scope:

| Client | Project signal | Global signal |
|--------|----------------|---------------|
| **Claude** | `<workspace>/.claude` **or** `<workspace>/.mcp.json` (a grim-managed MCP config is still a real Claude footprint, even without `.claude/`) | native root (`$CLAUDE_CONFIG_DIR` or `~/.claude`) exists **or** the sibling `.claude.json` MCP config exists |
| **OpenCode** | `<workspace>/.opencode` **or** the resolved project `opencode.json`/`.jsonc` exists (the same file grim manages for both rules and MCP entries) | native skills root (`$OPENCODE_CONFIG_DIR` or `$XDG_CONFIG_HOME/opencode/skills`) exists **or** the resolved global `opencode.json` (`$OPENCODE_CONFIG` / XDG default) exists |
| **Copilot** | a Copilot-specific marker — **not** bare `.github` (nearly every repo carries it for CI): `<workspace>/.github/copilot-instructions.md` or `<workspace>/.github/instructions/` — **or** `<workspace>/.vscode/mcp.json` exists | native skills root (`$COPILOT_HOME/skills` or `~/.copilot/skills`) exists — the `skills/` subdir, not the bare `~/.copilot` parent — **or** the global `mcp-config.json` exists |
| **Codex** | `<workspace>/.codex` — **not** the shared `.agents/skills` dir (a weak cross-vendor marker, like Copilot's bare `.github` caveat) | native config root (`$CODEX_HOME` or `~/.codex`) exists |
| **Cursor** | `<workspace>/.cursor` exists | `~/.cursor` exists |
| **Kiro** | `<workspace>/.kiro` exists | native root (`$KIRO_HOME` or `~/.kiro`) exists |
| **Junie** | `<workspace>/.junie` exists | `~/.junie` exists |
| **Gemini** | `<workspace>/.gemini` exists — **not** the shared `.agents/skills` dir (weak cross-vendor marker, like Codex) | native root (`$GEMINI_CLI_HOME/.gemini` or `~/.gemini`) exists |
| **Zed** | `<workspace>/.zed` exists | the platform-resolved Zed config root exists (`$XDG_CONFIG_HOME`\|`~/.config`/`zed` on Linux/FreeBSD, `~/.config/zed` on macOS, `%APPDATA%\Zed` on Windows) |
| **Amp** | `<workspace>/.amp` exists | `$XDG_CONFIG_HOME`\|`~/.config`/`amp` exists |
| **agents** | **never** — `false` at both scopes, by design | **never** |
| **Antigravity** | **never** — `false` by design; all its project surfaces live under the shared `.agents/`, so keying on it would install Antigravity into every workspace that ever used a pool client. Upstream documents no product-specific project marker | `~/.gemini/config` exists |
| **Cline** | `<workspace>/.clinerules` **or** `<workspace>/.cline` exists | `~/.cline` exists |
| **Droid** | `<workspace>/.factory` exists | `~/.factory` exists |
| **Goose** | `<workspace>/.goose` exists — **never** `.agents/`, which is where Goose writes | any of Goose's config roots exists (`$GOOSE_PATH_ROOT`, the XDG dir, and the macOS Application Support path, OR-ed) |
| **Warp** | `<workspace>/.warp` exists | `~/.warp` exists (identical on all three platforms) |
| **OpenClaw** | **never** — it has no project scope | `~/.openclaw` exists |
| **Kilo** | `<workspace>/.kilo` **or** `<workspace>/.kilocode` exists | `~/.config/kilo` or `~/.kilo` exists (OR-ed) |

**Never key `detect()` on `.agents/`.** It is a shared multi-client marker,
and for Goose — which *renders into* the pool — keying on it would make the
client detect itself after its own first install. Asserted per vendor.
Where several candidate markers are OR-ed (Cline, Kilo, Goose), that is
deliberate: detection writes nothing, so OR-ing risks only a missed
autodetect, never a wrong path. A *write* path would have to resolve to
exactly one location first.

**Known cross-client leak, disclosed not fixed.** Antigravity's global root
`~/.gemini/config` nests *inside* `~/.gemini`, which is Gemini CLI's global
marker — so a global Antigravity install creates a directory that makes
**gemini** detected on the next autodetected global command, and it
survives uninstall (grim removes files, not empty directories). The reverse
is clean: a Gemini install creates `~/.gemini/agents`, never `config/`.
Fixing it means narrowing `vendor_gemini`'s marker to a Gemini-CLI-exclusive
file — a shipped client's detection change under the freeze. Documented in
`docs/src/clients.md` `{#gap-antigravity}` and watchlisted.

A client whose only footprint on a machine/workspace is a grim-installed
MCP entry still counts as detected — its config file lives outside the
vendor's skills/root marker for every client above except Codex (whose
`config.toml` sits inside the directory `detect()` already checks, so no
extra clause is needed there).

Detection lives on the [`Vendor`] trait (`Vendor::detect(workspace,
scope)`), driven by `install::target::detect_clients`, which iterates
`ClientTarget::ALL` so the set is deterministic and returns the **raw**
detected set (possibly empty).

Two callers, two answers, and the split is load-bearing:

- **`InstallTarget::parse`** — the seam every mutating command uses.
  Nothing detected ⇒ the single generic `agents` client (`.agents/skills`,
  skills only). *Not* all clients: that old fallback wrote one directory
  per known vendor, and those directories were exactly what made the next
  run "detect" every client — it was not idempotent with respect to its own
  input. `AgentsVendor::detect` returns `false` at both scopes **by
  design**; the generic client is selected, never detected, so writing the
  pool changes no future resolution. Do not "fix" that to return `true`.
  A recorded `agents` output is likewise treated as unconditionally active
  when reconciling state, since it can only ever have been selected.

  **The surviving exit-78.** When that fallback is active **and** the
  artifact set holds nothing the generic client can install (only rules,
  agents, and/or MCP — all declined by it), the command exits **78**. The
  guard is `installer::refuse_uninstallable_fallback`, called as the first
  statement of `install_and_persist`, so it fires before any blob is
  fetched. It quantifies over `effective_supporting_clients` (target ∪
  recorded clients whose output still resolves), not over
  `target.clients()` alone — otherwise `install` would refuse a state that
  `update` re-materializes cleanly. The message names **both** `--client`
  and `[options].clients`, because `grim add` reaches the same seam and has
  no `--client` flag. `grim update` and `refresh_dev_installs` bypass it
  deliberately: they re-materialize recorded state, and guarding there
  would make an existing exit-0 path start erroring.
- **`detect_clients_or_all`** — the permissive wrapper for read-only
  consumers (`status`, `search`, the TUI badge sites), which reconcile
  *recorded* outputs against "which clients might be present". Nothing
  detected ⇒ all clients, unchanged from before. Also what
  `InstallTarget::new`'s empty-list branch still does — `new` is test-only
  (`parse` guarantees a non-empty list before delegating), but it is still
  `pub`, so a production caller passing `vec![]` would silently re-create
  the original bug.

**Zero detected is neither "all clients" nor an error.** Both readings were
true at different points and both are now wrong: the fallback is one
synthetic skills-only client writing the shared pool, and 78 survives only
in the narrow uninstallable case above.

An explicit `[options].clients` and the `--client` flag both override
detection. The detected set is **not** persisted to config — it is
recomputed each run, with one exception: `grim init` seeds
`[options].clients` from detection (writing no `[options]` table when
detection is empty, so the generic fallback is never persisted).
Detection reuses the same vendor env overrides documented in the table
above.

## Constraints

- Never assume a path operation crosses filesystems silently — check first.
- Treat the content store as append-only / immutable; mutate only the
  name → content mapping.
- Concurrent processes must coordinate via advisory file locks for any
  read-modify-write on shared metadata.

## Cross-References

- `arch-principles.md` — overall architecture and utility discipline
- `quality-security.md` — path traversal / symlink-escape guards (two-layer containment pattern)

<!-- external -->
[uv]: https://docs.astral.sh/uv/
[pixi]: https://pixi.sh/
