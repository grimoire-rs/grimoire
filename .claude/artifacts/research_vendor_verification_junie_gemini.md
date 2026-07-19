# Research: Junie + Gemini CLI Surface Verification (wave-1 vendor expansion)

Live verification 2026-07-19 (worker-researcher, primary sources only) for
[`adr_vendor_wave_expansion.md`](./adr_vendor_wave_expansion.md) ⚠ items.
Sibling artifacts: `research_vendor_verification_cursor_kiro.md`,
`research_vendor_verification_zed_amp.md`.

## 🚨 ADR contradictions

1. **Junie `.junie/rules/<name>.md` does NOT exist** — the earlier
   landscape claim was a hallucinated/stale search summary. Real surface
   (junie.jetbrains.com/docs/guidelines-and-memory.html, 2026-07-15):
   `.junie/AGENTS.md` (primary) → root `AGENTS.md` → legacy
   `.junie/guidelines.md` / `.junie/guidelines/` (folder semantics
   undocumented). Global `~/.junie/AGENTS.md`. Merge: global+project both
   included, marked; project wins; identical deduped. No glob scoping.
   **Resolution: Junie rules moves to the wave-2 AGENTS.md-injection
   bucket. Wave-1 Junie = skills + MCP; rules Declined (no ownable
   per-file surface).**
2. **Both vendors have real subagent formats the ADR declared absent:**
   - Junie: `.junie/agents/*.md` or `.agents/*.md` (project),
     `~/.junie/agents/` or `~/.agents/` (user); frontmatter `description`
     required + 8 optional fields — **EAP-only, not GA**
     (junie-cli-subagents.html, 2026-07-15). **Decline stands, reason
     corrected to "EAP-only"; watchlist for GA.**
   - Gemini: `.gemini/agents/*.md` (project) / `~/.gemini/agents/*.md`
     (user); `name`+`description` required; optional `kind`, `tools`,
     `model`, `temperature`, `max_turns`, `timeout_mins`; shipped
     non-EAP, gated only by `settings.json` `experimental.enableAgents`
     kill-switch (geminicli.com/docs/core/subagents/, 2026-06-08).
     **Kill-switch default RESOLVED (follow-up, 2026-07-19): `true`** —
     `packages/cli/src/config/settingsSchema.ts` on main
     (`enableAgents: { ..., default: true }`), pinned via revert PR
     #23672 (merged 2026-03-24, commit `055ff92`) restoring default-on;
     no later revert as of 2026-07-19. **→ Gemini Agent cell ships
     Native.** Re-verify at V2 tranche pre-flight (a `false` flip
     reverts the cell to Declined).

## Junie facts (junie.jetbrains.com, pages pinned 2026-07-15)

| Fact | Value | Matches ADR |
|---|---|---|
| Skill dirs | project `.junie/skills/<name>/`, global `~/.junie/skills/<name>/`; project overrides same-name user skill | Yes (⚠ resolved) |
| Skill frontmatter | `name` required, `description` optional (falls back to first body paragraph) — universal only | Yes |
| Rules | see contradiction 1 | **No** |
| Agents | see contradiction 2 (EAP) | **Reason corrected** |
| MCP paths | project `.junie/mcp/mcp.json` (VCS-shareable), user `~/.junie/mcp/mcp.json`, key `mcpServers` | Yes |
| MCP schema | local `command`+`args`+`env`; remote `url`+`headers` | Yes |
| MCP env refs | **not documented — treat as absent**, skip `${VAR}`-bearing descriptors | ⚠ resolved conservative |
| Detection | `.junie/` project marker, `~/.junie/` global | Yes |
| Env overrides | per-kind `JUNIE_{SKILL,AGENT,MCP,COMMAND}_LOCATIONS` (+`_DEFAULT_LOCATIONS`), `JUNIE_CONFIG_LOCATION`, `JUNIE_GUIDELINES_FILENAME` (environment-variables.html) | Richer than ADR — **not honored wave 1, watchlist** |

## Gemini CLI facts (geminicli.com + google-gemini/gemini-cli, fetched 2026-07)

| Fact | Value | Matches ADR |
|---|---|---|
| Skill dirs | workspace `.gemini/skills/` OR `.agents/skills/` alias; user `~/.gemini/skills/` OR `~/.agents/skills/` (docs 2026-04-30) | Yes (⚠ resolved) + shares `.agents/skills` anchor with Codex/Zed/Amp |
| Skill precedence | built-in < extension < user < workspace; same tier: `.agents/skills` beats `.gemini/skills` | New |
| Rules | GEMINI.md hierarchy only (user/project/component-level by directory) — no per-file glob rules; Declined confirmed | Yes |
| `context.fileName` | renames context file(s); open reliability bugs #19872, #7339, #9689 | Wave-2 injection caveat |
| Commands | `.gemini/commands/*.toml` = prompt files (`prompt`+`description`), confirmed not subagents | Yes |
| Agents | see contradiction 2 | **No — real format** |
| MCP path | project `.gemini/settings.json`, user `~/.gemini/settings.json`, key `mcpServers` | Yes |
| MCP entry schema | one of `command` (stdio) / `url` (SSE) / `httpUrl` (HTTP streaming) required; optional `args`, `headers`, `env`, `cwd`, `timeout`, `trust`, `includeTools`, `excludeTools`, `authProviderType`, `targetAudience`, `targetServiceAccount`, `oauth:{enabled}` | Deeper — **`url` vs `httpUrl` distinction must map from grim transport (sse→url, http→httpUrl)** |
| MCP env refs | `$VAR` / `${VAR}` POSIX all platforms, `%VAR%` Windows-only; undefined → empty string | Confirmed |
| Settings precedence | Default < system-defaults < user < project < system-override < env < CLI args | New |
| Detection | `.gemini/` project, `~/.gemini/` global; `GEMINI_CONFIG_DIR` **does not exist** (open FR #2815) | Yes |

## Implementation directives distilled

- Junie wave 1: skills (universal) + MCP (json splice, skip env-ref
  descriptors) only; rules + agents Declined with corrected reasons.
- Gemini transport mapping: grim `sse` → `url`, grim `http` → `httpUrl`,
  stdio → `command`; oauth-bearing descriptors skip (Gemini's
  `oauth:{enabled}`/`authProviderType` shape ≠ grim's structured block —
  watchlist possible partial mapping).
- Gemini agent cell: pending `experimental.enableAgents` default.
- No config-dir env override honored for either vendor in wave 1.

## Sources

junie.jetbrains.com/docs/{agent-skills,guidelines-and-memory,junie-cli-subagents,junie-cli-mcp-configuration,environment-variables}.html,
geminicli.com/docs/{cli/skills,cli/gemini-md,cli/custom-commands,core/subagents,tools/mcp-server,reference/configuration},
github.com/google-gemini/gemini-cli issues #19872, #7339, #9689, #2815.
