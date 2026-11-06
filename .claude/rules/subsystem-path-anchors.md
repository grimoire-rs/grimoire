---
paths:
  - src/install/path_anchor.rs
---

# Path Anchors

The PathAnchor set and the anchor root/remainder table. Index: [subsystem-file-structure.md](./subsystem-file-structure.md).

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
| `ClaudeUserDir` | `claude-user-dir` | the effective `CLAUDE_CONFIG_DIR` (shell or Claude settings `env`) else `$HOME` — the dir holding Claude's user config file `.claude.json`. A *second*, differently-shaped root for one vendor, so it cannot be a `VendorRoot` row: with the override set the file lives *inside* it, without it the file is a *sibling* of `~/.claude` |
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
| `claude` | `claude-root` | the effective `CLAUDE_CONFIG_DIR` (shell or Claude settings `env`) else `~/.claude` — `AnchorRoots::resolve` feeds the row the settings-aware value |
| `copilot` | `copilot-root` | `$COPILOT_HOME` else `~/.copilot` |
| `codex` | `codex-root` | `$CODEX_HOME` else `~/.codex` (hosts Codex `agents/` **and** the MCP `config.toml`) |
| `cursor` | `cursor-root` | `~/.cursor` (`CURSOR_CONFIG_DIR` not honored; hosts skills, `.mdc` rules, agents, `mcp.json`) |
| `kiro` | `kiro-root` | `$KIRO_HOME` else `~/.kiro` (hosts skills, `steering/` rules, `settings/mcp.json`) |
| `junie` | `junie-root` | `$JUNIE_HOME` else `~/.junie` (hosts skills, `agents/`, `mcp/mcp.json`) |
| `gemini` | `gemini-root` | `$GEMINI_CLI_HOME/.gemini` else `~/.gemini` — segment appended either way (hosts `agents/`, `settings.json` MCP; skills use `AgentsSkills`) |
| `zed` | `zed-root` | Linux/FreeBSD: `$XDG_CONFIG_HOME` else `~/.config`, then `/zed`; macOS: literal `~/.config/zed`; Windows: `%APPDATA%\Zed` (hosts `settings.json` MCP; skills use `AgentsSkills`) |
| `amp` | `amp-root` | `$XDG_CONFIG_HOME` else `~/.config`, then `/amp` (hosts `settings.json` MCP; skills use `AgentsSkills`) |
| `antigravity` | `antigravity-root` | `~/.gemini/config` — nested under, and distinct from, the `gemini` row. Each client's candidate set holds only its own root, so the nesting never cross-classifies |
| `cline` | `cline-root` | `~/.cline` |
| `droid` | `droid-root` | `~/.factory` — **the tag follows the CLIENT name, the directory follows the vendor's.** Both are frozen and they differ on purpose (hosts skills, `droids/` agents, `mcp.json`) |
| `warp` | `warp-root` | `~/.warp` (identical on macOS, Linux and Windows, deliberately so upstream; hosts skills, `.mcp.json` MCP) |
| `openclaw` | `openclaw-root` | `$OPENCLAW_STATE_DIR`, else `$OPENCLAW_HOME/.openclaw`, else `~/.openclaw` |
| `kilo` | `kilo-root` | `~/.kilo` (the legacy `.kilocode` is read for detection only, never written) |
| `kilo-config` | `kilo-config-root` | `$XDG_CONFIG_HOME` else `~/.config`, then `/kilo` — Kilo's **second** root, hosting its global `agents/`. The `opencode-config` precedent: one vendor, two roots, so the second row is not named after the vendor |
| `goose` | `goose-root` | `~/.goose` — hosts Goose's global `agents/` only; its skills anchor at `AgentsSkills` |
| `qoder` | `qoder-root` | `$QODER_CONFIG_DIR` else `~/.qoder` (hosts skills, rules, agents, `settings.json` MCP) |

`goose`'s skills render into the shared pool at both scopes and anchor at
`AgentsSkills`; its `goose-root` row exists for agents alone. The
vendor-neutral `agents` client has no row.

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
| project · any · any **except `hook`** | `workspace` | `.claude/…`, `.opencode/…`, `.github/…`, `.agents/…`, `.codex/…` (full sub-path from workspace) |
| project · any · hook | `grim-home` | `hooks/payload/<sha256-of-workspace>/<name>` — **the one project triple that does not anchor at `workspace`** (I1). `workspace` stays in the candidate set, second, so pre-relocation records still classify for the layout reaper |
| global · any · hook | `grim-home` | `hooks/<name>` (client-independent — S-003) |
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
| project · any · mcp | `workspace` | client-specific config path from the [MCP table](./subsystem-install-layout-agents-mcp.md#install-layout-mcp) (`.mcp.json`, `.cursor/mcp.json`, `.kiro/settings/mcp.json`, …); entry-typed output — `entry` carries the managed member's JSON pointer |
| global · claude · mcp | `claude-user-dir` | `.claude.json` |
| global · opencode · mcp | `open-code-root` | `opencode.json` |
| global · copilot · mcp | `copilot-root` | `mcp-config.json` |
| global · codex · mcp | `codex-root` | `config.toml` (same root as the agent anchor — TOML splice, see [install-layout-mcp](./subsystem-install-layout-agents-mcp.md#install-layout-mcp)) |

**Every other vendor** follows the same `(scope, client, kind)` →
`(anchor, relative)` shape, with `<name>-root` as its global anchor and the
`relative` remainder taken verbatim from the Install Layout and
[Global-scope](./subsystem-vendor-paths.md#global-scope-paths) tables above — e.g. global · cursor ·
rule → `cursor-root` + `rules/<name>.mdc`; global · kiro · mcp →
`kiro-root` + `settings/mcp.json`. Three exceptions worth stating:

- **Global pool skills anchor at `agents-skills`, never at a vendor
  root** — Codex, Gemini, Zed, Amp, Goose, the generic `agents` client,
  and any pool reader whose skills joined the pool (auto-join beside a
  pool writer, or `shared_skills = true`). Goose's `goose-root` hosts only its agents. (At *project* scope every
  triple anchors at `workspace`, pool or not; the pool shows up only in
  the `relative` remainder, `.agents/skills/<name>`.)
- **Antigravity is a partial pool member.** Its *project* skills pool into
  `.agents/skills`; its *global* skills anchor at
  `antigravity-root` (`~/.gemini/config/skills/<name>`). Nothing in the
  installer or prune assumes scope-uniform pool membership — both key on
  resolved destination paths, not on a roster. **Pool *capability* is
  scope-blind**, though: a partial member must not join
  `POOL_CAPABLE_VENDORS`, or a pool join would write global skills where the
  client never scans, and nothing would fail.
- **Pool membership classifies both layouts for one triple.**
  `candidate_anchors` cannot see the config or the peer set, so a pool-capable client's
  skill triple returns `[native root, agents-skills, grim-home]` — native
  first, so a root tie keeps the client's own anchor rather than silently
  re-anchoring a user whose skills never joined the pool.

Declined `(client, kind)` pairs are never classified — skipped at the
`kind_support` gate before anchoring. A pair declined only *at one scope*
(`Vendor::kind_surface(kind, scope) == false`, today Junie rules at global
and OpenClaw skills at project) is likewise never classified at that scope:
the installer warns, skips, and records zero outputs.
