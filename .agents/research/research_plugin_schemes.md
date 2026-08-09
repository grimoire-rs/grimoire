# Research: Harness Plugin/Extension Schemes — Feasibility of Grim-Owned Plugin Containers

**Date:** 2026-07-10
**Method:** Deep-research fan-out (105 agents: 5 search angles → 23 sources
fetched → 95 claims extracted → top 25 adversarially verified with 3-vote
panels, ≥2/3 refutations kill a claim → 17 confirmed → 8 synthesized
findings). All confirmed findings cite official vendor docs current as of
mid-2026 (VS Code 1.110, Claude Code v2.1.x).
**Question:** Can grim maintain grim-owned, locally-managed plugin/extension
containers per AI coding harness, with optional namespacing and partial
add/remove of contents — instead of (or alongside) today's vendor-native
directory rendering?

---

## Executive Summary

1. **The plugin-container strategy is viable exactly where the namespacing
   feedback comes from: Claude Code.** Colon namespacing
   (`plugin-name:component`) is documented and consistent across skills,
   commands, and agents. Plugins register via an `enabledPlugins` key in
   three documented settings scopes — user (`~/.claude/settings.json`),
   project (`.claude/settings.json`), local (`settings.local.json`) — so
   grim's project/global scope split maps 1:1 onto native surfaces.
2. **VS Code Copilot agent mode is a strong second target.** Its
   `plugin.json` carries commands, skills, agents, hooks, and MCP servers in
   any combination; enable/disable state is stored separately from install
   state. Independently, `chat.agentSkillsLocations` lets a third party
   register an extra managed skills directory without touching vendor trees
   — a documented config-splice point tailor-made for a grim-owned dir.
3. **Copilot CLI blocks partial add/remove inside plugins.** Official docs:
   `/skills remove` does not work on plugin-bundled skills; the plugin is
   the unit of lifecycle. A grim container there cannot support per-skill
   mutation — keep loose file-drop rendering (current approach) for
   Copilot CLI.
4. **OpenCode and Codex CLI offer no usable container surface.** OpenCode
   plugins are npm/Bun JS packages (code, not config); registering a purely
   local filesystem plugin dir is unconfirmed. Codex CLI's only verified
   extensibility is MCP-server registration in `config.toml`; a claimed
   `skills.config` path mechanism was refuted 3-0.
5. **Mutation is never fully passive.** Claude Code hot-reloads a skill's
   `SKILL.md` text mid-session, but hooks/MCP/agents/output-styles need an
   explicit `/reload-plugins` or restart — grim must surface that after
   relevant installs. VS Code toggles at plugin granularity.
6. **Everything churns.** VS Code's own docs already drift from shipped
   behavior (manifest location documented at plugin root, actually required
   under `.github/`). Claude's marketplace plumbing
   (`known_marketplaces.json`, `extraKnownMarketplaces`) remains
   undocumented. Every surface must be re-verified hands-on at build time.

**Net for grim:** plugin containers are a **per-vendor projection mode**
(exactly the slot `adr_render_layout_stability.md` reserved), not a new
universal model. Classic vendor-native rendering stays the baseline for
Copilot CLI, OpenCode, and Codex CLI.

---

## Confirmed Findings (survived 3-vote adversarial verification)

### Claude Code

| # | Finding | Confidence | Sources |
|---|---|---|---|
| 1 | Plugins namespaced by plugin name with colon notation for skills/commands and agents (`plugin-dev:agent-creator`, `/commit-commands:commit`) — native mechanism grim can map package identity onto | High (3-0, 2-1) | [plugins-reference], [discover-plugins] |
| 2 | Three install scopes — user `~/.claude/settings.json`, project `.claude/settings.json`, local `.claude/settings.local.json` — registered via `enabledPlugins` key at the corresponding scope | Medium (2-1 splits despite exact doc quotes) | [plugins-reference], [discover-plugins] |
| 3 | Mutation asymmetric, non-live: `SKILL.md` text hot-reloads mid-session; hooks/, `.mcp.json`, agents/, output-styles/ require explicit `/reload-plugins` or restart. No continuous re-scan | High (3-0 ×2) | [plugins-reference], [discover-plugins] |

### VS Code Copilot agent mode

| # | Finding | Confidence | Sources |
|---|---|---|---|
| 4 | `plugin.json` supports component pluralism (slash commands, agent skills, custom agents, hooks, MCP servers in any combination); enable/disable state stored separately from plugin configuration — components removable from availability without uninstall/re-registration. Shipped VS Code 1.110 | High (3-0 ×2) | [vscode-agent-plugins] |
| 5 | Skills discovered from six fixed locations — project: `.github/skills/`, `.claude/skills/`, `.agents/skills/`; personal: `~/.copilot/skills/`, `~/.claude/skills/`, `~/.agents/skills/` — plus `chat.agentSkillsLocations` setting to register extra skill directories | High (3-0, 2-1) | [vscode-agent-skills] |

### GitHub Copilot CLI

| # | Finding | Confidence | Sources |
|---|---|---|---|
| 6 | Skills use `SKILL.md` (YAML frontmatter: required `name`, `description`; optional `allowed-tools`), but plugin-bundled skills cannot be individually removed or mutated — "To remove skills added as part of a plugin you must manage the plugin itself." Direct blocker for per-skill add/remove inside a grim container | High (3-0, 2-1) | [copilot-cli-skills] |

### OpenCode (SST)

| # | Finding | Confidence | Sources |
|---|---|---|---|
| 7 | Plugins discovered from project `.opencode/plugins/` and global `~/.config/opencode/plugins/`; config merges across sources rather than replacing; plugins also registered as npm package refs in config (`"plugin": ["opencode-helicone-session", …]`), installed via Bun at startup. Registration path is npm/JS-package-centric, not a generic content directory — local non-npm dir registration **unconfirmed** | Medium (2-1 ×3) | [opencode-config], [opencode-plugins] |

### OpenAI Codex CLI

| # | Finding | Confidence | Sources |
|---|---|---|---|
| 8 | Verified extensibility limited to MCP-server registration (`[mcp_servers.<id>]` in `config.toml`) and tool-namespace arrays (`features.code_mode.direct_only_tool_namespaces` / `excluded_tool_namespaces`). No skills/agents/rules directory mechanism confirmed; a claimed `skills.config` path mechanism was **refuted 3-0** | Medium (2-1 ×2) | [codex-config] |

---

## Capability Matrix — plugin-container fit per harness

| Harness | Container scheme | Namespacing | Project scope | Global scope | Partial add/remove in container | Verdict |
|---|---|---|---|---|---|---|
| Claude Code | `.claude-plugin/` plugin + marketplace | ✅ `plugin:component` | ✅ `.claude/settings.json` `enabledPlugins` | ✅ `~/.claude/settings.json` | ✅ skills live; ⚠️ hooks/MCP/agents need `/reload-plugins` | **Feasible — primary target** |
| VS Code Copilot agent | `plugin.json` (under `.github/`) | scope-based | ✅ workspace settings + `chat.agentSkillsLocations` | ✅ user profile paths | ⚠️ plugin-level toggle; skills-dir splice bypasses container entirely | **Feasible — secondary; skills-dir splice may suffice** |
| Copilot CLI | plugin bundles | prefix on plugin skills | file-drop only | file-drop only | ❌ plugin is lifecycle unit | **Not feasible — keep file-drop** |
| OpenCode | npm/Bun JS plugin packages | n/a (code plugins) | config merge friendly | config merge friendly | n/a — plugins are code, not config artifacts | **Not applicable — keep `opencode.json` splice** |
| Codex CLI | none verified | n/a | `config.toml` MCP only | `config.toml` MCP only | n/a | **No surface — MCP splice only** |
| Cursor, Gemini CLI, Windsurf, Cline, Roo, Amp, Zed | — | — | — | — | — | **Uncovered** — no claims survived this pass (Gemini CLI extension docs fetched but unverified); needs second sweep |

## Integration-Method Matrix

| Harness | File-drop | Config-splice | Marketplace/registry | Env var |
|---|---|---|---|---|
| Claude Code | plugin dir contents (grim-owned root) | `enabledPlugins` in settings.json (3 scopes, documented) | `known_marketplaces.json` / `extraKnownMarketplaces` — **undocumented plumbing, spike required** | `CLAUDE_CONFIG_DIR` (already honored by grim) |
| VS Code Copilot | six fixed skill dirs (incl. `.claude/skills/` — reads Claude's layout) | `chat.agentSkillsLocations` (documented) | n/a | — |
| Copilot CLI | skill dirs (current grim path) | none documented | plugin install commands only | `COPILOT_HOME` (already honored) |
| OpenCode | `.opencode/plugins/` (JS code only) | `opencode.json` merge (current grim path) | npm registry | `OPENCODE_CONFIG` (already honored) |
| Codex CLI | — | `config.toml` `[mcp_servers.*]` | — | — |

---

## Refuted / Unconfirmed Claims (treat as *unconfirmed*, not false)

Adversarial verification defaults to refute on uncertainty; several kills
are over-refutation of compound claims. Notable:

- "`.claude-plugin/plugin.json` manifest with components incl. LSP servers,
  monitors, themes" — refuted 0-3. **The manifest path itself is in the
  official docs**; the exotic component list almost certainly killed the
  compound claim. Authoritative `plugin.json` schema + component-type list
  remains an open question (two competing specific claims both died).
- VS Code hierarchical manifest fallback (`.claude-plugin/` → `.plugin/` →
  root) — refuted 0-3; discovery path unconfirmed (docs say root, shipped
  behavior `.github/`).
- Copilot CLI "session cache + `/skills reload`" — refuted 0-3; actual
  reload behavior unconfirmed.
- OpenCode manifest-less TS-file scanning — refuted 0-3.
- Codex CLI `skills.config` path mechanism — refuted 3-0 (likely genuinely
  absent).

## Open Questions

1. Native schemes for Cursor, Gemini CLI, Windsurf, Cline, Roo Code, Amp,
   Zed — entirely uncovered; needed to complete the matrices.
2. Claude Code's authoritative `plugin.json` schema and component-type
   list — resolve hands-on, not via web research.
3. Does Claude Code pick up new plugins in an already-registered local
   marketplace without re-registration, and what exactly must be spliced
   (`enabledPlugins` per plugin? marketplace entry once)? — the core spike
   question for a grim-owned container.
4. Can OpenCode register a purely local, non-npm plugin directory, or must
   a grim-managed plugin be packaged as an npm module?
5. Does Codex CLI have any skill/agent/rules directory mechanism at all,
   or is `config.toml` MCP registration genuinely its only surface?

## Implications for Grim

- **Confirms the hybrid architecture.** Plugin container = per-vendor
  projection mode, the slot `adr_render_layout_stability.md` explicitly
  reserved ("a mode on `ClaudeVendor`", grim-owned source roots). Classic
  rendering stays the universal baseline.
- **Namespace design**: optional namespace resolvable per artifact >
  bundle > catalog/registry > project default > none (classic render),
  mirroring the existing registry-precedence pattern. Namespace = plugin
  name = toggle granularity in both Claude Code and VS Code.
- **Mutation contract**: after installing hooks/MCP/agents into a Claude
  plugin container, grim must instruct `/reload-plugins` (or restart).
  Skill-text-only updates are live.
- **Install-state fit**: grim-owned plugin dirs are ordinary render targets
  — `ClientOutput` paths, expected-bytes hashes, prune, uninstall all apply
  unchanged; only the marketplace/`enabledPlugins` registration needs the
  reversible config-splice pattern already proven for MCP.
- **Next step**: hands-on spike against live Claude Code — local
  marketplace registration, `enabledPlugins` splice, reload behavior —
  before any ADR acceptance. Web research cannot resolve the undocumented
  plumbing.

## Sources (primary, load-bearing)

- [plugins-reference]: https://code.claude.com/docs/en/plugins-reference
- [discover-plugins]: https://code.claude.com/docs/en/discover-plugins
- [vscode-agent-plugins]: https://code.visualstudio.com/docs/agent-customization/agent-plugins
- [vscode-agent-skills]: https://code.visualstudio.com/docs/agent-customization/agent-skills
- [copilot-cli-skills]: https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/add-skills
- [opencode-config]: https://opencode.ai/docs/config/
- [opencode-plugins]: https://opencode.ai/docs/plugins/
- [codex-config]: https://developers.openai.com/codex/config-reference

Secondary/context (fetched, lower weight): Gemini CLI extension docs
(geminicli.com/docs/extensions/reference, google-gemini/gemini-cli repo,
developers.googleblog.com), MCP release blog, assorted comparison blogs
(quality: blog/unreliable — used for harness-landscape scoping only).
