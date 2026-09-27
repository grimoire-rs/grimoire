# Research: Plugin/Pack Format Compatibility Across Harnesses (batch C)

<!--
Technology-landscape research. Filename and location: this project's
documented research convention; `.agents/research/research_[topic].md`
if undocumented.
Owner: a researcher worker. Handoff to: /hex-architect, /hex-plan.

Purpose: persist landscape findings that inform ADRs, plans, and design
decisions. Findings decay - check the Expires date before trusting them.
-->

## Metadata

**Date:** 2026-09-27
**Domain:** packaging
**Triggered by:** /hex-discuss harness-native-marketplaces — plugin format interop check
**Expires:** 2027-03-27

## Direct Answer

Of the nine harnesses, five have a native mechanism that installs a *collection*
of agent config (skills + agents + commands + hooks + MCP + rules) as one unit:
**Google Antigravity** (Agent Plugins, `plugin.json`), **JetBrains Junie**
(Extensions, `marketplace.json` + per-extension manifest), **OpenClaw**
(Plugin bundles via ClawHub, multi-format), **Qoder** (Plugins bundling
skills/agents/commands/rules/MCP), and **Sourcegraph Amp** partially (directory
plugins can bundle a Skill package plus code, but with no manifest/registry).
**Cline** and **Warp** are rules/workflow-only with no bundling manifest.
**Block Goose** is MCP-only for its "extension" concept (skills are a separate,
unbundled mechanism). **Zed** extensions cannot carry skills/agents/hooks/rules
at all — languages, themes, debuggers, snippets, MCP servers only.

Only **Junie** and **OpenClaw** are documented as reading Claude Code's
`.claude-plugin/` format natively (both explicitly, no conversion step
claimed). Antigravity's `plugin.json` shape is structurally close to Claude's
but no doc claims direct read-compatibility.

## Findings Table

| Harness | Mechanism | Manifest + dir | Registry/transport | Components | Version semantics | Reads `.claude-plugin/`? |
|---|---|---|---|---|---|---|
| Sourcegraph Amp | yes (directory "plugin", code-first) | no formal manifest found; dir `.amp/plugins/<name>/` (project) or `~/.config/amp/plugins/` (global); TS entrypoint registers a bundled Skill package from a subdir | none — manually placed, no registry/marketplace documented | commands, tools, UI prompts, event hooks (`session.start`, `tool.call`, …), **skills** (bundled Agent Skill packages), agent modes, webhooks, link patterns | undocumented (no manifest version field found) | no — not mentioned |
| Google Antigravity | yes — "Agent Plugins" | `plugin.json` at plugin root (required fields: `name`, optional `description`, optional `$schema`); optional `mcp_config.json`, `hooks.json`, `skills/`, `agents/`, `rules/` | file-based: place folder in `.agents/plugins/` (workspace) or `~/.gemini/config/plugins/` (global); also a bundled "Customizations panel" install path; `agy plugin` CLI (list/install/enable/disable/uninstall) | skills (`SKILL.md`), agents (subagent/persona `.md`), rules (`.md`), MCP config, hooks | undocumented (no version field or update-compare semantics found) | not documented either way |
| Cline | **skills-only-ish, no bundling manifest** — rules + workflows + skills are separate loose-file mechanisms | none — plain markdown in `.clinerules/` or `.cline/rules/` (workspace) or global dir; no package manifest | community GitHub repo `cline/clinerules` browsed via in-app "Prompts Library"; MCP servers separately via `cline/mcp-marketplace` (submission-reviewed GitHub issues, README + optional `llms-install.md`, one-click install) | rules, workflows, skills, MCP servers — each independently stored, never packaged together | none — no version field, no marketplace manifest | no — not mentioned |
| Block Goose | **MCP-only** for the "extension" concept; skills are a separate, unbundled feature | extension = a `config.yaml` entry (fields: `bundled`, `enabled`, `name`, `timeout`, `type`) under `~/.config/goose/config.yaml`; no manifest bundles rules/agents/hooks with it | no marketplace found for "extensions" in these docs; skills auto-discovered from `~/.config/goose/skills/` (or shared `~/.claude/skills/`) | extensions = MCP servers only (tools/resources/prompts, 6 transport types: builtin/stdio/http/streamable-http/etc.); skills = separate `SKILL.md` dirs, not part of an extension package | none documented for extensions; skills use plain YAML frontmatter, no version field found | no — not mentioned; notably *shares* the skills directory path with Claude Desktop (`~/.claude/skills/`) as a convenience, not a plugin-format read |
| JetBrains Junie | yes — "Extensions" | native: `.junie-extension/marketplace.json`; **also reads** `.claude-plugin/marketplace.json` directly | git repos (GitHub/GitLab/self-hosted), local dirs, or HTTP(S) manifest URLs; built-in official marketplace hosted on JetBrains' GitHub | agent skills, subagents, MCP servers, custom slash commands, guidelines, hooks | undocumented in fetched pages (per-extension manifest filename/version-compare semantics not found — flagging as a gap, not inferring) | **yes** — explicit: "supports both native Junie and Claude plugin formats" for marketplace listings |
| OpenClaw (ClawHub) | yes — "Plugin bundles" + "ClawPack" | marker files per format: native `plugin.json` at root; also detects `.codex-plugin/plugin.json`, `.claude-plugin/plugin.json` (or manifestless Claude layout), `.cursor-plugin/plugin.json` | ClawHub = hosted registry (clawhub.ai); transport = npm-pack `.tgz` artifact, digest-verified; semver-tagged (`latest`, changelogs, stars, security scan) | skills (all formats), commands (Claude/Cursor), agents + output styles (Claude), hooks (Claude/Codex, mapped to `HOOK.md`), MCP servers (all), LSP servers (Claude), settings (Claude `settings.json`), rules (Cursor — "detected but not executed") | semver; `package.json` must declare `openclaw.compat.pluginApi` + `openclaw.build.openclawVersion`; one entry tracks many semver versions + tags | **yes, directly** — "no conversion step required," reads `.claude-plugin/plugin.json` and manifestless Claude layouts natively |
| Qoder | yes — "Plugins" (superset of Skills) | manifest filename/dir not found in fetched page (gap); a plugin is "a folder that combines multiple Skills, role instructions, and MCP dependencies" | in-app "Marketplace" (Quest sidebar), browse by category; also `+ Create Plugin` flow via built-in `plugin-creator` skill; user-level or project-level install scope | skills (`SKILL.md`-based), MCP servers, agents, commands, rules, hooks (components are managed as a unit — cannot toggle individually per docs) | undocumented (no version field/update semantics found) | not documented either way |
| Warp | **rules/workflow-only, no bundling manifest** | none found — Rules, Workflows, MCP servers, Skills are separate Warp Drive object types, no combined manifest | Warp Drive (team-shared knowledge objects); no packaging/marketplace/transport documented for a multi-component "pack" | rules, workflows (parameterized command sequences), MCP servers, skills — each a distinct Warp Drive object type, applied uniformly across app/CLI/cloud, cited as "References"/"Derived from" | none documented | no — not mentioned |
| Zed | **no** | `extension.toml` (fields: `id`, `name`, `version`, `schema_version`, `authors`, `description`, `repository`; a git repo is the unit) | central `zed-industries/extensions` git repo (`extensions.toml` index) + Zed's in-app extension browser; dev-extension local install also supported | languages, grammars (tree-sitter, via repo+rev), language servers, themes, icon themes, snippets, debuggers, **MCP servers** — explicitly **not** skills, agents/subagents, slash commands, hooks, or rules | `version` field present in `extension.toml`, semver-shaped by convention in examples; comparison/update semantics not documented in fetched page | no — no capability surface exists to carry Claude-shaped config in the first place |

## Key findings

1. Two harnesses claim direct Claude-plugin-format interop: JetBrains Junie
   reads `.claude-plugin/marketplace.json` alongside its native
   `.junie-extension/marketplace.json` ([Junie CLI extensions
   docs](https://junie.jetbrains.com/docs/junie-cli-extensions.html)), and
   OpenClaw's ClawHub reads `.claude-plugin/plugin.json` (manifest or
   manifestless) with "no conversion step required"
   ([OpenClaw plugin bundles](https://docs.openclaw.ai/plugins/bundles)).
2. Google Antigravity's `plugin.json` + `skills/`/`agents/`/`rules/`/
   `mcp_config.json`/`hooks.json` directory shape ([Antigravity plugins
   docs](https://antigravity.google/docs/plugins/)) is structurally the
   closest analog to Claude Code's `.claude-plugin/` layout among the
   remaining harnesses, but no fetched doc claims Antigravity reads Claude's
   format or vice versa.
3. "Extension" is overloaded: Goose's "extension" is MCP-server-only
   ([Goose config-files guide](https://goose-docs.ai/docs/guides/config-files/)),
   while Junie's and Antigravity's "extension"/"plugin" is a full multi-component
   bundle — the same word names a materially different capability set per
   vendor.
4. Zed is the one harness in this set with zero surface for skills, agents,
   hooks, commands, or rules in its extension system — `extension.toml`
   ([Zed developing-extensions
   docs](https://zed.dev/docs/extensions/developing-extensions)) only
   declares languages, debuggers, themes, snippets, and MCP servers.
5. Version-field semantics (semver vs. equality-compared vs. precedence, one
   version per registry entry vs. many) are documented in detail only for
   OpenClaw/ClawHub (semver + tags + changelog, many versions per package
   entry) and loosely implied for Zed (a bare `version` string in
   `extension.toml`, no compare semantics stated). All other harnesses in
   this set leave version semantics undocumented in what was fetched.

## negative:

- Cline: no manifest, no version field, no marketplace for a bundled
  "pack" of rules+workflows+skills+MCP — confirmed absent, not merely
  unfound, per [Cline rules docs](https://docs.cline.bot/customization/cline-rules)
  ("no mention of versioning, manifests, or marketplace infrastructure").
- Warp: no bundling manifest, no marketplace/registry, no version semantics
  for a multi-component pack — [Warp agent-mode-context
  docs](https://docs.warp.dev/knowledge-and-collaboration/warp-drive/agent-mode-context/)
  explicitly has "no information available" on packaging/distribution as a
  unit.
- Zed: no skills/agents/commands/hooks/rules capability exists in the
  extension system at all — confirmed by the declared capability list
  (languages, debuggers, themes, icon themes, snippets, MCP servers) in
  [Zed developing-extensions docs](https://zed.dev/docs/extensions/developing-extensions).
- Amp, Antigravity, Qoder: no marketplace/registry-with-transport
  documented for their bundling mechanism (Amp: fully manual local
  placement, no registry at all; Antigravity: bundled + manual + CLI, but
  no hosted registry described; Qoder: an in-app Marketplace exists but
  its transport — git/zip/npm/hosted-API — is not stated in the fetched
  page).
- No harness in this set was found to read Claude's format only
  "partially" — every case resolved to a clean yes/no in what was
  fetched; "partial" reads (e.g., skills recognized but rules ignored)
  were not observed for Claude-format specifically (contrast: OpenClaw
  does read Cursor rules only partially — "detected but not executed" —
  but that is a different source format, not Claude's).

## leads:

- Junie per-extension manifest filename (distinct from the
  `marketplace.json` listing file) and its version-compare semantics were
  not found in the two Junie pages fetched — the extension-authoring/
  packaging page (not yet located) likely has this; worth a follow-up
  fetch of `junie.jetbrains.com/docs/` navigation for an
  "extensions"/"packaging" page beyond `agent-skills.html` and
  `junie-cli-extensions.html`.
- Qoder's plugin manifest filename/directory convention and version field
  were not in `docs.qoder.com/extensions/plugins`; `docs.qoder.com/plugins/introduction`
  and `docs.qoder.com/qoderwork/skill-marketplace-guidelines` (seen in
  search results, not fetched) likely carry the manifest schema.
- Amp version/update semantics and whether toolboxes (a separate,
  non-plugin mechanism per `ampcode.com/news/toolboxes`) also bundle rules
  or hooks was not explored — only the Plugin API and directory-plugin
  path were fetched; the toolbox mechanism is a distinct, unresearched
  surface.
- Google Antigravity's read-compatibility with `.claude-plugin/` (or lack
  thereof) was not confirmed either way by the fetched plugins page —
  worth a direct check given how close the two directory shapes are.

## Sources

| Source | Type | Date | Relevance |
|--------|------|------|-----------|
| https://ampcode.com/manual/plugin-api | Docs | fetched 2026-09-27 | Amp Plugin API: components, directory structure, no manifest/registry |
| https://ampcode.com/news/toolboxes | Blog (not fetched, found via search) | n/a | Amp toolboxes — separate mechanism, unresearched |
| https://antigravity.google/docs/plugins/ | Docs | fetched 2026-09-27 | Antigravity Agent Plugin manifest, directory layout, install methods |
| https://docs.cline.bot/customization/cline-rules | Docs | fetched 2026-09-27 | Confirms no manifest/marketplace for Cline rules bundling |
| https://github.com/cline/mcp-marketplace | Repo/README (search summary) | n/a | Cline MCP marketplace submission process, README-based, no manifest |
| https://github.com/cline/clinerules | Repo (search summary) | n/a | Community rules/workflows/skills library, browsed via Prompts Library |
| https://goose-docs.ai/docs/guides/config-files/ | Docs (search summary) | n/a | Goose extension config.yaml fields; skills as separate discovery mechanism |
| https://junie.jetbrains.com/docs/junie-cli-extensions.html | Docs | fetched 2026-09-27 | Junie Extensions: bundle contents, marketplace hosting, Claude-format read support |
| https://junie.jetbrains.com/docs/agent-skills.html | Docs | fetched 2026-09-27 | Junie Agent Skills SKILL.md format; no manifest/version detail found |
| https://docs.openclaw.ai/plugins/bundles | Docs | fetched 2026-09-27 | OpenClaw bundle format detection across 4 ecosystems incl. Claude, component mapping table |
| https://docs.openclaw.ai/clawhub | Docs | fetched 2026-09-27 | ClawHub registry transport (npm-pack .tgz), semver/tags/changelog versioning |
| https://docs.qoder.com/extensions/plugins | Docs | fetched 2026-09-27 | Qoder plugin components, marketplace, install scope; manifest filename not found |
| https://docs.warp.dev/knowledge-and-collaboration/warp-drive/agent-mode-context/ | Docs | fetched 2026-09-27 | Confirms no bundling manifest/marketplace for Warp Drive objects |
| https://zed.dev/docs/extensions/developing-extensions | Docs | fetched 2026-09-27 | Zed extension.toml schema, declared capability list excludes skills/agents/hooks/rules |
