---
paths:
  - src/install/target.rs
  - src/install/client_target.rs
---

# Client Detection

How default install targets are detected. Index: [subsystem-file-structure.md](./subsystem-file-structure.md).

## Client Detection (default install targets) {#client-detection}

When neither `--client` nor the config `[options].clients` selects a
client, `install` / `update` / TUI target **all detected clients**. A
client is detected when its vendor directory / config marker is present
for the active scope:

| Client | Project signal | Global signal |
|--------|----------------|---------------|
| **Claude** | `<workspace>/.claude` **or** `<workspace>/.mcp.json` (a grim-managed MCP config is still a real Claude footprint, even without `.claude/`) | native root (`$CLAUDE_CONFIG_DIR` or `~/.claude`) exists **or** the sibling `.claude.json` MCP config exists |
| **OpenCode** | `<workspace>/.opencode` **or** the resolved project `opencode.json`/`.jsonc` exists (the same file grim manages for both rules and MCP entries) | native skills root (`$OPENCODE_CONFIG_DIR` or `$XDG_CONFIG_HOME/opencode/skills`) exists **or** the resolved global `opencode.json` (`$OPENCODE_CONFIG` / XDG default) exists |
| **Copilot** | a Copilot-specific marker — **not** bare `.github` (nearly every repo carries it for CI): `<workspace>/.github/copilot-instructions.md` or `<workspace>/.github/instructions/` — **or** `<workspace>/.vscode/mcp.json` or `<workspace>/.github/mcp.json` exists (the CLI's own file, as Copilot-specific as the VS Code one) | native skills root (`$COPILOT_HOME/skills` or `~/.copilot/skills`) exists — the `skills/` subdir, not the bare `~/.copilot` parent — **or** the global `mcp-config.json` exists |
| **Codex** | `<workspace>/.codex` — **not** the shared `.agents/skills` dir (a weak cross-vendor marker, like Copilot's bare `.github` caveat) | native config root (`$CODEX_HOME` or `~/.codex`) exists |
| **Cursor** | `<workspace>/.cursor` exists | `~/.cursor` exists |
| **Kiro** | `<workspace>/.kiro` exists | native root (`$KIRO_HOME` or `~/.kiro`) exists |
| **Junie** | `<workspace>/.junie` exists | native root (`$JUNIE_HOME` or `~/.junie`) exists |
| **Gemini** | `<workspace>/.gemini` exists — **not** the shared `.agents/skills` dir (weak cross-vendor marker, like Codex) | native root (`$GEMINI_CLI_HOME/.gemini` or `~/.gemini`) exists |
| **Zed** | `<workspace>/.zed` exists | the platform-resolved Zed config root exists (`$XDG_CONFIG_HOME`\|`~/.config`/`zed` on Linux/FreeBSD, `~/.config/zed` on macOS, `%APPDATA%\Zed` on Windows) |
| **Amp** | `<workspace>/.amp` exists | `$XDG_CONFIG_HOME`\|`~/.config`/`amp` exists |
| **agents** | **never** — `false` at both scopes, by design | **never** |
| **Antigravity** | **never** — `false` by design; all its project surfaces live under the shared `.agents/`, so keying on it would install Antigravity into every workspace that ever used a pool client. Upstream documents no product-specific project marker | `~/.gemini/config` exists |
| **Cline** | `<workspace>/.clinerules` **or** `<workspace>/.cline` exists | `~/.cline` exists |
| **Droid** | `<workspace>/.factory` exists | `~/.factory` exists |
| **Goose** | `<workspace>/.goose` exists — **never** `.agents/`, which is where Goose writes | any candidate config root exists: `$XDG_CONFIG_HOME/goose` or `~/Library/Application Support/goose`, OR-ed — **but `$GOOSE_PATH_ROOT`, when set, *replaces* that list rather than extending it.** `~/.goose` is not a global marker |
| **Warp** | `<workspace>/.warp` exists | `~/.warp` exists — deliberately the same path on all three platforms; the OS-specific app-data dirs are **not** consulted |
| **OpenClaw** | **never** — it has no project scope, and `kind_surface(Skill, Project)` refuses skills there too | native root (`$OPENCLAW_STATE_DIR`, `$OPENCLAW_HOME/.openclaw`, or `~/.openclaw`) exists |
| **Kilo** | `<workspace>/.kilo` **or** `<workspace>/.kilocode` exists — `.kilocode` counts for detection only and is **never written** | `~/.kilo` or `$XDG_CONFIG_HOME/kilo` exists (OR-ed) |
| **Qoder** | `<workspace>/.qoder` exists (its `settings.json` MCP file sits inside it, so no extra clause) | native root (`$QODER_CONFIG_DIR` or `~/.qoder`) exists |

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
`docs/src/content/docs/clients.md` `{#gap-antigravity}` and watchlisted.

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

  When the fallback is active, `install_and_persist` logs one `warn` naming
  the pool and both selection knobs (issue #113) — the fallback is the
  right default for pool readers but silent misplacement for Claude Code.
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
