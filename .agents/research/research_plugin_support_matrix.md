# Research: Plugin support across grim-supported harnesses

## Metadata

**Date:** 2026-09-27
**Domain:** packaging
**Triggered by:** /hex-discuss harness-native-marketplaces — which of grim's 18 client targets can take an exported plugin, and in what shape
**Expires:** 2027-03-27

Consolidation of three lanes; every cell traces to one of them, which carry
the URLs:
`.agents/research/research_plugin_format_compat_a.md` (Claude Code, Copilot, Codex),
`.agents/research/research_plugin_format_compat_b.md` (Cursor, Gemini CLI, OpenCode, Droid, Kilo, Kiro),
`.agents/research/research_plugin_format_compat_c.md` (Amp, Antigravity, Cline, Goose, Junie, OpenClaw, Qoder, Warp, Zed).

## Direct Answer

There is no single plugin standard. The one real cross-vendor standard is
**Agent Skills (`SKILL.md`)** — the payload. At the plugin layer, three
manifest families exist, plus harnesses with no plugin mechanism at all:

1. **Claude shape** — `.claude-plugin/plugin.json` + `.claude-plugin/marketplace.json`.
   Native in Claude Code; read directly by Droid (translation layer), Junie
   (marketplace), OpenClaw (plugin); partially by VS Code Copilot.
2. **Agent Plugins 1.0** — verified (`research_agent_plugins_spec_verify.md`):
   independent open spec at agent-plugins.org, 1.0.0 published 2026-08-06,
   maintainers Amazon, Cursor, Microsoft, OpenAI, Vercel, plus Google; **not
   Anthropic, not an AAIF project**. Root `plugin.json` (required `$schema`,
   `name`; `version` optional, semver recommended) with components
   **`skills/` and `mcp.json` only** — agents, commands, hooks, rules
   deferred to later versions. Confirmed implementers: Copilot, Codex,
   Cursor (alongside its own `.cursor-plugin/`). Gemini CLI unconfirmed;
   OpenCode refuted (open request); Kiro, Antigravity rest on secondary
   sources only.
3. **Vendor-own** — Cursor (`.cursor-plugin/`), Gemini CLI
   (`gemini-extension.json`), Kilo (`PLUGIN.yaml`), Qoder (undocumented).

One Claude-shape folder therefore does **not** reach every plugin-capable
harness; a per-family manifest is required.

## Matrix — grim client targets

| Harness | Plugin mechanism | Manifest | Marketplace / registry | Rules in plugin | Reads Claude format | Version semantics |
|---|---|---|---|---|---|---|
| Claude Code | yes | `.claude-plugin/plugin.json` | `.claude-plugin/marketplace.json`; git, url, npm, zip archive, local | **no** (plugin `CLAUDE.md` rejected) | native | plain string equality; one version per entry; pin via ref/SHA/sha256 |
| Copilot (CLI, VS Code, app) | yes (Agent Plugins 1.0) | root `plugin.json` | `.github/plugin/marketplace.json`, falls back to `.claude-plugin/` | Copilot-namespaced only | partial (VS Code) | undocumented ("informational" per non-primary source) |
| Codex | yes (Agent Plugins 1.0) | root `plugin.json` (`.codex-plugin/` legacy) | workspace GitHub marketplaces; official directory not self-serve | **no** (AGENTS.md separate) | no | undocumented |
| Cursor | yes | `.cursor-plugin/plugin.json` | `.cursor-plugin/marketplace.json`, git only documented | yes, `rules/*.mdc` (open `alwaysApply` bug) | no | optional string; update = git ref refresh, not version compare |
| Gemini CLI | yes ("extensions") | `gemini-extension.json` | none; gallery + `extensions install <git\|path>` | yes (`contextFileName`, `GEMINI.md`) | no | "should match release tag"; `--ref` pin |
| OpenCode | code only (JS/TS npm module) | none | npm | undocumented | partial (scans `.claude/skills/`) | npm semver |
| Droid | yes | `.factory-plugin/plugin.json` | git / npm / path | no | **yes** (explicit translation) | from marketplace entry |
| Kilo Code | marketplace (skills, MCP, agents, plugins) | `PLUGIN.yaml` | `Kilo-Org/kilo-marketplace` | yes (`.kilocode/rules/`) | no | undocumented |
| Kiro | yes ("Powers") | `plugin.json` (Agent-Plugins-aligned) or legacy `POWER.md` | kiro.dev/powers, GitHub URL | yes (steering) | no | undocumented |
| Junie | yes ("extensions") | `.junie-extension/marketplace.json` | git, local, HTTP manifest URL | undocumented | **yes** (marketplace) | undocumented |
| OpenClaw | yes (bundles, ClawPack) | root `plugin.json`; detects `.claude-`, `.codex-`, `.cursor-plugin/` | ClawHub, npm-pack `.tgz` | undocumented | **yes** | semver, many versions per package |
| Antigravity | yes ("Agent Plugins") | root `plugin.json` + `skills/ agents/ rules/ mcp_config.json hooks.json` | file-based | yes (`rules/`) | undocumented | undocumented |
| Qoder | yes | undocumented | in-app marketplace | yes (role instructions) | undocumented | undocumented |
| Amp | dir-based, no manifest | — | none | — | no | — |
| Goose | MCP-only extensions | `config.yaml` entries | — | no | no | — |
| Cline | no | — | — | — | — | — |
| Warp | no | — | — | — | — | — |
| Zed | no (extensions cover languages/themes/MCP only) | — | — | — | — | — |

## Commonalities

- `SKILL.md` payload and a `skills/` directory: identical in every plugin-capable harness.
- The filename `plugin.json` is shared by Claude, Copilot, Codex, Cursor, Droid, Kiro, OpenClaw, Antigravity — the **location** differs (`.claude-plugin/`, `.cursor-plugin/`, `.factory-plugin/`, root).
- Transport is git almost everywhere; npm for OpenCode, OpenClaw; zip only Claude.
- No harness has a lockfile; versions are change markers, not resolution inputs (OpenClaw, OpenCode excepted — npm semver).

## Differences that matter for export

- **Manifest location/family** — at least three shapes (Claude, root `plugin.json`, vendor-own).
- **Rules** — absent in Claude Code, Codex, Droid; present in Cursor, Gemini, Kilo, Kiro, Antigravity, Qoder; vendor-namespaced in Copilot.
- **MCP file name** — `.mcp.json` (Claude), `mcp.json` (Agent Plugins), `mcp_config.json` (Antigravity), inline in `gemini-extension.json`.
- **Agent format** — Markdown+frontmatter (Claude family) vs `droids/` (Droid) vs TOML (Codex native agents).
- **Zip upload** — only Anthropic: claude.ai skill upload (30 MB, per user) and Claude Code `archive` marketplace source. None documented for Copilot or Codex plugins.

## negative:

- Qoder manifest filename, Junie per-extension manifest, Kilo/Kiro namespacing and versioning: not found in fetched primary docs.
- Copilot and Codex version-comparison algorithm: undocumented.

## leads:

- A single folder with several manifests side by side (`.claude-plugin/`, root `plugin.json`, `.cursor-plugin/`, `gemini-extension.json`) over one shared `skills/` tree — untested whether each harness ignores the others' files.

## Sources

See the three lane artifacts listed under Metadata; this file adds no new sources.
