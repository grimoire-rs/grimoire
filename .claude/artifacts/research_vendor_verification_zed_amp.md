# Research: Zed + Amp Surface Verification (wave-1 vendor expansion)

Live verification 2026-07-19 (worker-researcher, primary sources only) for
[`adr_vendor_wave_expansion.md`](./adr_vendor_wave_expansion.md) ⚠ items.
Sibling artifacts: `research_vendor_verification_cursor_kiro.md`,
`research_vendor_verification_junie_gemini.md`.

## 🚨 ADR corrections

1. **Zed rules are NOT "AGENTS.md only" — 9-file first-match precedence:**
   `.rules` → `.cursorrules` → `.windsurfrules` → `.clinerules` →
   `.github/copilot-instructions.md` → `AGENT.md` → `AGENTS.md` →
   `CLAUDE.md` → `GEMINI.md` (zed docs/src/ai/instructions.md, main,
   fetched 2026-07-19). Declined verdict holds (no scoping anywhere), but
   **wave-2 injection must target the first existing file or document
   shadowing** — a managed AGENTS.md block is silently dead in any project
   carrying `.rules`/`.cursorrules`.
2. **Amp has a real project marker: `.amp/` (workspace `settings.json`
   merged over global)** — ampcode.com/news/cli-workspace-settings
   (2025-11-04). ADR's "AGENTS.md too generic, detection weak" concern is
   stale. Also: **`$AMP_SETTINGS_FILE` not found in primary docs** — only
   the `--settings-file` CLI flag; do not implement the env var.

## Zed facts (zed-industries/zed main docs, fetched 2026-07-19)

| Fact | Value | Matches ADR |
|---|---|---|
| Skills scan | project `<worktree>/.agents/skills/` + global `~/.agents/skills/`; **flat only** (nested subfolders not discovered); no Zed-native dir | Yes |
| Rules | native Rules removed in v1.4 (→ Skills); instruction files per 9-name precedence above; personal `~/.config/zed/AGENTS.md`; project file overrides personal | Correction 1 |
| MCP key | `context_servers` in settings.json | Yes |
| MCP entry shape | **flat**: local `{command, args, env}`; remote `{url, headers}` (nested `command:{path,...}` shape is stale-blog-only) | ⚠ resolved |
| MCP env refs | **unsupported** — literal values only (open discussions #26043, #18630, #56881, #53780); skip `${VAR}`-bearing descriptors | ⚠ resolved |
| Settings paths | global `~/.config/zed/settings.json` (honors `$XDG_CONFIG_HOME`; Windows `%APPDATA%\Zed\`); project `.zed/settings.json`; JSONC (`//` comments) confirmed | Yes |
| Detection | `.zed/` project marker; `~/.config/zed/` global; no config-dir env override found | Yes |

## Amp facts (ampcode.com manual + news, fetched 2026-07-19)

| Fact | Value | Matches ADR |
|---|---|---|
| Skills scan | workspace `.agents/skills/`; user `~/.config/agents/skills/` + `~/.agents/skills/`; compat `~/.config/amp/skills/`, `.claude/skills/`, `~/.claude/skills/`; name-collision precedence documented (all scanned) | Yes |
| Rules | `AGENTS.md` → `AGENT.md` → `CLAUDE.md` (three names only, not Zed's long list); no scoping → Declined wave 1 holds | Yes |
| @-mention + globs | confirmed: mentioned files with `globs:` frontmatter load lazily on match — wave-2 scoped-injection mechanism | Yes |
| MCP settings | global `~/.config/amp/settings.json`/`.jsonc` (Windows `%USERPROFILE%\.config\amp\`); **workspace `.amp/settings.json`/`.jsonc` merged over global** | Path confirmed; workspace tier new |
| MCP key | `"amp.mcpServers"` — literal single dotted JSON key (not nested), pointer-safe | Yes |
| MCP entry schema | local `command`/`args`/`env`; remote `url`/`headers`; common `includeTools` (glob array) | Yes |
| MCP env refs | `${VAR_NAME}` supported inside values | Yes |
| Settings override | `--settings-file` CLI flag only; `$AMP_SETTINGS_FILE` unverified — **do not implement** | Correction 2 |
| Detection | project `.amp/`; global `~/.config/amp/`; AGENTS.md only weak secondary | Correction 2 |

## Implementation directives distilled

- Zed + Amp wave 1: skills via shared `.agents/skills` anchor (already
  written for Codex — refcount guard required), MCP via json_splice
  (JSONC-tolerant), rules + agents Declined.
- Amp project-scope MCP targets `.amp/settings.json` (workspace tier);
  global targets `~/.config/amp/settings.json`.
- Zed: skip env-ref-bearing MCP descriptors entirely (no expansion
  upstream — Copilot-CLI-global precedent). Amp: translate refs to
  `${VAR_NAME}` (identity from canonical `${VAR}`).
- Watchlist rows: Zed 9-file precedence (wave-2 shadowing), Zed env-ref
  discussions, Amp `$AMP_SETTINGS_FILE` absence, Amp skills-precedence list.

## Sources

raw.githubusercontent.com/zed-industries/zed/main/docs/src/ai/{instructions,skills,mcp}.md,
zed docs/src/configuring-zed.md, docs.open.cx/mcp/clients/zed,
github.com/zed-industries/zed discussions #26043 #18630 #56881 #53780,
ampcode.com/manual, ampcode.com/news/{cli-workspace-settings,agent-skills}.
