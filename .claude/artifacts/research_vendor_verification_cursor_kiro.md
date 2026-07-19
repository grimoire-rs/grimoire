# Research: Cursor + Kiro Surface Verification (wave-1 vendor expansion)

Live verification 2026-07-19 (worker-researcher, primary sources only) for
[`adr_vendor_wave_expansion.md`](./adr_vendor_wave_expansion.md) ⚠ items.
Sibling artifacts: `research_vendor_verification_junie_gemini.md`,
`research_vendor_verification_zed_amp.md`.

## 🚨 ADR contradictions

1. **Cursor agents are NOT declined-worthy.** Cursor v2.4 (2026-01-22)
   shipped file-based subagents: `.cursor/agents/<name>.md` project,
   `~/.cursor/agents/` global; markdown + YAML frontmatter (`name`,
   `description`, `model` [default "inherit"], `readonly` [bool],
   `is_background` [bool]); body = system prompt. Cursor also reads
   `.claude/agents/` and `.codex/agents/` (`.cursor/` wins conflicts);
   v2.5 added nested launching. Sources:
   cursor.com/docs/context/subagents, cursor.com/changelog/2-4.
   **Resolution: flip Cursor Agent cell to Native.**
2. **Kiro agents: native IDE format exists but decline stands for a new
   reason.** `.kiro/agents/` (workspace) / `~/.kiro/agents/` (user),
   markdown + frontmatter (`description`, `model`, `tools` tags,
   `mcpServers` inline, permission rules) — kiro.dev/docs/custom-agents/
   (2026-07-01, "Kiro 0.9"). **Landmine:** Kiro CLI expects an
   incompatible JSON agent schema in the SAME directory — open upstream
   bug kirodotdev/Kiro#8040. Writing IDE-format files could break
   Kiro-CLI users. **Resolution: keep Declined, corrected reason
   (format collision), watchlist entry; re-verify before wave 2.**

## Cursor facts (all cursor.com docs, fetched 2026-07-19; version pins where dated)

| Fact | Value | Matches ADR |
|---|---|---|
| Skill project dir | `.cursor/skills/<name>/SKILL.md` (also scans universal `.agents/skills/`) | Yes |
| Skill global dir | `~/.cursor/skills/` (also `~/.agents/skills/`) | Yes (⚠ resolved) |
| Skill frontmatter | universal + Cursor extras: `paths` (glob scope), `disable-model-invocation`, `metadata` | New — potential future `cursor.*` skill registry, not wave 1 |
| Skill name constraint | `[a-z0-9-]`, must match folder name | New |
| Rules | `.cursor/rules/*.mdc`, nested folders OK; plain `.md` ignored; `.cursorrules` deprecated-but-working | Yes |
| Rules frontmatter | `description` (string), `globs` (**comma-separated STRING, not array**), `alwaysApply` (bool) | Type detail new — pins serialization |
| Agents | `.cursor/agents/<name>.md` — see contradiction 1 | **No** |
| MCP paths | project `.cursor/mcp.json`, global `~/.cursor/mcp.json`, key `mcpServers` | Yes |
| MCP local schema | `type: "stdio"` (required), `command`, `args`, `env`, `envFile` | Adds `type`/`envFile` |
| MCP remote schema | `url`, `headers`, `auth` (OAuth: CLIENT_ID/CLIENT_SECRET/scopes) | New — OAuth surface exists (shape ≠ grim McpOAuth; skip descriptors wave 1) |
| MCP env refs | `${env:NAME}` + `${userHome}`, `${workspaceFolder}`, `${workspaceFolderBasename}`, `${pathSeparator}` | ⚠ resolved |
| Detection | `.cursor/` project marker; `~/.cursor` global. `CURSOR_CONFIG_DIR` documented only on CLI reference — **may be CLI-only; do not honor wave 1** | ⚠ care |

## Kiro facts (kiro.dev docs; page dates noted)

| Fact | Value | Matches ADR |
|---|---|---|
| Skill dirs | project `.kiro/skills/`, global `~/.kiro/skills/` (docs 2026-02-18) | Yes (⚠ resolved) |
| Skill frontmatter | `name` (req ≤64), `description` (req ≤1024), `license`, `compatibility`, `metadata` | Yes |
| Steering `inclusion` | **four** literals: `always` (default), `fileMatch` (+`fileMatchPattern` string OR array), `manual`, `auto` (requires `name`+`description`) | ADR missed `auto` |
| Global steering | `~/.kiro/steering/` | Yes |
| Global fileMatch bug | kirodotdev/Kiro#9176 **still open** (checked 2026-07-19; #6171 closed duplicate) | Resolution (plan review, supersedes earlier skip+warn): write correct `fileMatch` at both scopes; global output inert-with-render-warning until upstream fix |
| MCP paths | workspace `.kiro/settings/mcp.json`, user `~/.kiro/settings/mcp.json`, merged (workspace wins), key `mcpServers` (docs 2026-06-10) | Yes |
| MCP local schema | `command`, `args`, `env`, `disabled`, `autoApprove`, `disabledTools` | Yes |
| MCP remote schema | `url`, `headers`, `env`, `oauth` (`clientId`,`redirectUri`), `oauthScopes`, + common fields | Remote confirmed; oauth shape ≠ grim block — skip wave 1 |
| MCP env refs | `${VARIABLE_NAME}` (simple form) | ⚠ resolved |
| Agents | see contradiction 2 — Declined, collision reason (#8040); CLI JSON schema at kiro.dev/docs/cli/custom-agents/configuration-reference/ (2026-07-02) | **Reason corrected** |
| Detection | `.kiro/` project, `~/.kiro/` global. `KIRO_HOME` is **CLI-only; IDE hardcodes `~/.kiro` and ignores it** (open bug #9148) — do not honor wave 1 | ⚠ care |

## Implementation directives distilled

- Cursor rule transform: `paths` → `globs` comma-join (same helper family as
  Copilot `applyTo`) + `alwaysApply: false`; unscoped → `alwaysApply: true`.
- Cursor agent registry candidates: `cursor.model`, `cursor.readonly`
  [bool], `cursor.is-background` [bool].
- Kiro steering transform: scoped → `inclusion: fileMatch` +
  `fileMatchPattern` (array form); unscoped → `inclusion: always` (default —
  consider omitting frontmatter entirely when unscoped).
- Kiro global-scope scoped rules (resolution updated at plan review):
  write correct `fileMatch` steering at both scopes; global output is
  inert until upstream #9176 closes — render-layer warning from
  `rule_index`, Known-gaps row. (Earlier "skip + warn / scope-aware
  KindSupport" directive superseded — the case is content-dependent,
  invisible to a kind-level gate.)
- Neither `CURSOR_CONFIG_DIR` nor `KIRO_HOME` honored in wave 1; both
  watchlisted with the upstream ambiguity/bug refs.

## Sources

cursor.com/docs/{skills,context/rules,context/subagents,mcp,cli/reference/configuration},
cursor.com/changelog/2-4, cursor.com/help/customization/rules,
kiro.dev/docs/{skills,steering,mcp/configuration,custom-agents,cli/custom-agents/configuration-reference,chat/subagents},
kiro.dev/blog/custom-subagents-skills-and-enterprise-controls,
github.com/kirodotdev/Kiro issues #9176, #6171, #8040, #9148.
