# Research: Plugin/Extension Format Interop Across AI Coding Harnesses

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

Across the six harnesses surveyed, only the **Agent Skills `SKILL.md`
format** (folder + `SKILL.md` + optional `scripts/`/`references/`/`assets/`)
is a genuine, broadly-adopted cross-vendor standard — governed at
[github.com/agentskills/agentskills](https://github.com/agentskills/agentskills),
showcased as adopted by 40+ clients including Claude Code, Cursor,
GitHub Copilot, VS Code, Gemini CLI, OpenAI Codex, Kiro, Factory, and Roo
Code ([agentskills.io](https://agentskills.io)).

The **plugin/marketplace wrapper** (manifest file name + fields, registry
file, component-directory conventions beyond skills) is **not**
interoperable. Every harness invented its own manifest location and shape:
`.claude-plugin/plugin.json` (Claude Code), `.cursor-plugin/plugin.json`
(Cursor), `gemini-extension.json` at the extension root (Gemini CLI), no
manifest at all — a JS/TS module — for OpenCode plugins, `PLUGIN.yaml`
(Kilo Code), `plugin.json`/legacy `POWER.md` (Kiro). A newer vendor-neutral
effort, **Agent Plugins 1.0** (released 2026-08-06 by Amazon, Cursor,
Microsoft, OpenAI, Vercel;
[aaif.io/blog/from-skills-and-tools-to-portable-agent-plugins](https://aaif.io/blog/from-skills-and-tools-to-portable-agent-plugins)),
standardizes a manifest + fixed Skills/MCP component locations, but
**Anthropic is not a party to it** and Claude Code keeps its own distinct
`.claude-plugin/` format.

**Factory Droid is the only harness that explicitly reads and translates
Claude Code's plugin layout**: its docs state `.claude-plugin/` is
translated to `.factory-plugin/`, `agents/` to `droids/`, `.mcp.json` to
`mcp.json`
([docs.factory.ai/cli/configuration/plugins](https://docs.factory.ai/cli/configuration/plugins)).
Cursor, Gemini CLI, OpenCode, Kilo Code, and Kiro do **not** read
`.claude-plugin/` paths at all — an unmodified Claude Code plugin directory
would not install or load in any of them except Factory, and even there
only through Factory's own translation layer, not native Claude Code
loading.

**Always-on rules/instructions shipped inside a plugin**: supported by
Cursor (`rules/*.mdc`, though a live 2026 bug reports `alwaysApply: true`
from a plugin is not being auto-injected), Gemini CLI (`contextFileName`,
defaults to `GEMINI.md`, auto-loaded every session), and Kiro (steering
files are an explicit part of a Power bundle) — but **explicitly NOT
supported by Claude Code itself**: Anthropic's docs state a `CLAUDE.md` at
a plugin root is not loaded as context and `claude plugin validate` warns
if one is found; instructions must go in a skill instead. Factory has no
dedicated rules/context component either. OpenCode supports `AGENTS.md` as
always-on context with a documented `CLAUDE.md` fallback chain, but whether
a *plugin itself* (as opposed to the user's project/global config) can
ship one is undocumented.

## Per-Harness Comparison

| Harness | Manifest (file + dir) | Marketplace/registry | Source types | Component types | Always-on rules in plugin? | Namespacing | Claude-format cross-read |
|---|---|---|---|---|---|---|---|
| **Claude Code** (baseline) | `.claude-plugin/plugin.json`, plugin root; only `name` required | `.claude-plugin/marketplace.json`, root of marketplace | path, `github`, `url`, `git-subdir`, `npm`, `archive`, `command` | skills, commands, agents, hooks, `.mcp.json`, `.lsp.json`, output-styles, workflows, themes/monitors (experimental), `bin/` | **No** — plugin-root `CLAUDE.md` explicitly not loaded; validator warns; must use a skill | `plugin-name:command`, `plugin-name:agent`, `plugin:<plugin>:<server>` (MCP) | n/a (own format) |
| **Cursor** | `.cursor-plugin/plugin.json`, plugin root; only `name` required | `.cursor-plugin/marketplace.json`, marketplace root; git-hosted repos only documented | git repo (documented); npm/zip/url — undocumented | rules (`rules/*.mdc`), skills (Agent-Plugins-compatible), agents, commands, hooks, `mcp.json` | **Spec: yes** (`rules/*.mdc`, same frontmatter as project rules incl. `alwaysApply`) — **currently reported broken** for plugin-sourced rules (Cursor 3.0.16 forum bug reports) | Commands prefixed `/plugin-name:command`; **skills not namespaced** (tracked bug, same gap as Claude Code) | **None found** — no mention of `.claude-plugin`/Claude Code anywhere in Cursor docs or `cursor/plugins` repo |
| **Gemini CLI** | `gemini-extension.json`, extension root (`.gemini/extensions/<name>/` or `~/.gemini/extensions/<name>/`) | No registry file — central gallery at geminicli.com/extensions/browse; install via `gemini extensions install <git URL\|local path>` | git repo (any host), GitHub Releases archive, local path; no npm | MCP servers, custom commands (`commands/*.toml`), context file (`contextFileName`, default `GEMINI.md`), hooks (`hooks/hooks.json`), subagents (`agents/*.md`), skills (`skills/<name>/SKILL.md`), policies (`policies/*.toml`) | **Yes** — `contextFileName` auto-loads every session | Bare name; on collision auto-renamed `/<extension-name>.<command>` (dot separator; rename logic itself has an open 2026 bug report) | **No** — separate loaders; community `gemini extensions import` bridge proposal closed unimplemented; some repos mistagged `gemini-cli-extension` while only shipping `.claude-plugin/plugin.json`, confirming non-interop |
| **OpenCode** | No JSON manifest — plugin is a JS/TS module exporting a hooks function, from `.opencode/plugins/` or `~/.config/opencode/plugins/`, or `opencode.json` `"plugin"` array | None first-party/curated — plugin entries are npm specifiers | npm package (name/scope), local `file://` path, git via `name@git+https://…` (latter two confirmed only via a GitHub issue, not prose docs — docs gap) | plugin hooks (`tool.execute.*`, `session.*`, `file.*`, `shell.env`, `message.*`), agents (`.opencode/agent/`), commands (`.opencode/commands/`), skills (`SKILL.md` — reads OpenCode-native, `.claude/skills/`, **and** `.agents/skills/` paths, in that order), MCP servers (`opencode.json` `"mcp"`), `AGENTS.md`/`CLAUDE.md` project/global rules | **Undocumented for plugins specifically** — `AGENTS.md`/`CLAUDE.md` is always-on at project/global scope with a documented Claude-Code-compat fallback chain, but docs are silent on whether a *plugin package* can ship its own auto-injected instructions file | None documented — identity is directory/filename only; skill `name` must match its directory | **Partial** — does not read `.claude-plugin/plugin.json`/`marketplace.json` directly, but **shares the `SKILL.md` directory convention** by explicitly also scanning `.claude/skills/` and `~/.claude/skills/`; also has an explicit `~/.claude/CLAUDE.md` fallback for rules |
| **Factory Droid** (brief) | `.factory-plugin/plugin.json`, plugin root; droid "does not require or read the fields inside `plugin.json`" — identity comes from the marketplace entry | marketplace entry-based; git/npm/relative-path sources | git, npm (incl. scoped `@scope/plugin`), relative path | skills, commands, droids (subagents), output-styles, hooks, `mcp.json` | **No** dedicated rules/context component — closest is hook-based context injection | `pluginName@marketplaceName` | **Yes, explicit** — `.claude-plugin/` → `.factory-plugin/`, `agents/` → `droids/`, `.mcp.json` → `mcp.json` translation documented |
| **Kilo Code** (brief) | Marketplace repo [Kilo-Org/kilo-marketplace](https://github.com/Kilo-Org/kilo-marketplace): Skills (`SKILL.md`), MCP servers, Agents (`agents/`, legacy `modes/`), Plugins (`PLUGIN.yaml` under `plugins/<id>/`) | Central marketplace repo, categorized by domain | undocumented in sources reached | skills, MCP servers, agents, plugins (`PLUGIN.yaml`) | **Yes** — `.kilocode/rules/` (current convention, back-compat), consolidating toward shared `kilo.jsonc` | Undocumented | **Not found** — no mention of reading `.claude-plugin/` |
| **Kiro Powers** (brief) | `plugin.json` (current, Agent-Plugins-1.0-aligned) or legacy `POWER.md`, both supported | one-click from kiro.dev/powers, in-IDE, GitHub URL, or Cloud sync | git/GitHub URL, IDE catalog, cloud sync | `plugin.json` (activation keywords), `skills/` (SKILL.md), `mcp.json`, `dev.kiro/` steering files | **Yes** — steering files explicitly part of a Power bundle | Undocumented | **No explicit Claude Code mention** — Kiro instead adopted the vendor-neutral Agent Plugins 1.0 spec as its bridge, not a Claude-specific reader |

## Version Semantics

| Harness | `version` field | Update-check mechanism | Registry: multi- vs single-version | How a user pins an older version |
|---|---|---|---|---|
| **Claude Code** | Optional; resolution order plugin.json → marketplace entry → derived (git SHA / sha256 / npm version / `"unknown"`) | `claude plugin update`; background auto-update ~10 min into a session, gated by `autoUpdate` on the marketplace entry | Single current version per marketplace entry; history via git-source `ref`/`sha` | `github`/`url`/`git-subdir` sources: `ref` (branch/tag) and/or `sha` (40-char, wins if both set); `npm` source: `version`+`registry` fields ([code.claude.com/docs/en/plugins/loading#versions-and-updates](https://code.claude.com/docs/en/plugins/loading#versions-and-updates)) |
| **Cursor** | Optional, "e.g. `1.0.0`" — no documented semver validation ([cursor.com/docs/reference/plugins](https://cursor.com/docs/reference/plugins)) | **Not a manifest-version comparison at all** — marketplace "Refresh"/"Auto Refresh" re-indexes at most every 10 min from the tracked git branch; a live forum bug shows the real installed state is pinned to the git ref/commit captured at import time, with "no obvious UI to force upgrade or clear the pinned commit" ([forum.cursor.com](https://forum.cursor.com/t/add-plugin-github-imports-can-get-stuck-on-stale-plugin-versions/163895)) | Single version/source per marketplace entry, no release array; only `minClientVersions` exists (a per-client compatibility gate, not a release list) | **Undocumented/unsupported** — no `--version` flag, no `#tag` syntax in the `source` field; the observed pin-to-a-commit behavior is an import-time side effect (arguably a bug), not a supported downgrade feature |
| **Gemini CLI** | Present, "should match the release tag" ([geminicli.com/docs/extensions/releasing](https://geminicli.com/docs/extensions/releasing/)) — required/optional and validation not stated | **Install-type-dependent, and mostly bypasses the manifest field**: GitHub-Releases installs query the GitHub API for the latest tag ("ignores the `version` field... for detection"); git-clone installs run `git ls-remote` vs local HEAD; only local-path installs compare the manifest `version` string, and even there semver-precedence vs. plain inequality is undocumented | No registry data structure at all — each extension is a live git/Releases source; version history lives only in git tags/releases | Explicit **`--ref <branch\|tag\|commit>`** flag on `gemini extensions install`; no `--version` flag |
| **OpenCode** | No manifest version field — a plugin is an npm specifier in `opencode.json`'s `"plugin"` array; version resolution is npm's own semver machinery via the `npm-package-arg` library (confirmed in source, `packages/opencode/src/plugin/shared.ts`), defaulting to `"latest"` when unspecified — this is genuine semver-range handling but is **undocumented in prose docs**, a source-code-only finding | N/A — resolution happens at install/run time through npm's own resolver, not a separate "check for update" step | No registry — resolves straight through the npm registry, which is inherently multi-version/queryable by range or dist-tag (structurally the opposite of Cursor/Gemini's single-current-version entries) | npm semver range/pin directly in the specifier, e.g. `"pkg@1.2.3"` or `"pkg@^1.0.0"` (undocumented in prose, implemented via `npm-package-arg`); or a local `file://`/relative/absolute path; a git-URL specifier is plausible via the same library's `git+`/`github:` support but was not directly observed in this repo's code path in this pass |

## Key Findings

1. Only the Agent Skills `SKILL.md` format is a genuine, multi-vendor
   interop point — every other plugin surface (manifest shape, marketplace
   file, MCP-server config key, command/agent directory names) is
   harness-specific. [agentskills.io](https://agentskills.io)
2. Claude Code's plugin/marketplace format is explicitly excluded from the
   "Agent Skills" cross-vendor effort — that standard covers only the
   skill file, not the `.claude-plugin/` packaging wrapper. Anthropic's own
   docs make no interoperability claim for `plugin.json`/`marketplace.json`.
   [code.claude.com/docs/en/plugins-reference](https://code.claude.com/docs/en/plugins-reference)
3. Factory Droid is the sole harness surveyed with a documented,
   built-in translation layer for Claude Code's plugin directory layout
   (`.claude-plugin/` → `.factory-plugin/`, `agents/` → `droids/`,
   `.mcp.json` → `mcp.json`).
   [docs.factory.ai/cli/configuration/plugins](https://docs.factory.ai/cli/configuration/plugins)
4. A genuinely vendor-neutral plugin/marketplace standard now exists —
   **Agent Plugins 1.0** (2026-08-06, Amazon/Cursor/Microsoft/OpenAI/Vercel)
   — but Anthropic did not join it, so it does not close the gap with
   Claude Code.
   [aaif.io/blog/from-skills-and-tools-to-portable-agent-plugins](https://aaif.io/blog/from-skills-and-tools-to-portable-agent-plugins)
5. Namespacing/collision handling is immature industry-wide: Cursor's
   plugin-provided *skills* are not namespaced (a gap explicitly tracked
   against Claude Code's identical pattern:
   [anthropics/claude-code#50486](https://github.com/anthropics/claude-code/issues/50486)),
   and Gemini CLI's collision-rename logic has its own open 2026 bug
   ([deja-vu#3665](https://github.com/vshulcz/deja-vu/issues/3665)).
6. "Always-on rules shipped inside a plugin" is the widest behavioral split
   found: supported by design in Cursor, Gemini CLI, and Kiro; explicitly
   disallowed by design in Claude Code (must be a skill instead); absent as
   a component type in Factory; undocumented at the plugin level in
   OpenCode (though present at the project/global `AGENTS.md` level with a
   `CLAUDE.md` fallback).
7. Version semantics diverge sharply in mechanism, not just policy: Claude
   Code and OpenCode both do real semver/SHA-aware resolution (Claude Code
   via an explicit resolution order with `ref`/`sha` pinning; OpenCode via
   npm's own resolver) — but Cursor and Gemini CLI both track a git
   ref/commit rather than comparing the manifest `version` field at all,
   and neither documents semver-precedence comparison logic anywhere.
8. OpenCode's `SKILL.md` loader is the strongest structural interop point
   found in this survey: it scans `.opencode/skills/`, `.claude/skills/`,
   **and** `.agents/skills/` (project and global variants of each) in a
   documented fallback order, and separately falls back to
   `~/.claude/CLAUDE.md` for rules.
   [opencode.ai/docs/skills/](https://opencode.ai/docs/skills/)

## negative:

- No harness other than Factory reads `.claude-plugin/plugin.json` or
  `.claude-plugin/marketplace.json` paths — confirmed absent in Cursor,
  Gemini CLI, OpenCode, Kilo Code, and Kiro docs/repos searched.
- No published canonical JSON Schema URL was found for Claude Code's
  `plugin.json`/`marketplace.json` (the `$schema` field exists but no
  target URL is documented), nor for Cursor's `.cursor-plugin/plugin.json`
  (contrast: the separate, non-Cursor-native "Agent Plugins" standard does
  publish `https://agent-plugins.org/schemas/1.0.0/plugin.schema.json`),
  nor for Gemini CLI's `gemini-extension.json`.
- No harness's marketplace/registry file format supports listing multiple
  selectable versions of one plugin as a queryable array — Claude Code,
  Cursor, and Gemini CLI all resolve to exactly one current
  version/ref per entry; OpenCode has no registry file at all (bypassed by
  npm's inherent multi-version registry).
- Searched specifically and found no formal member list or governance
  detail beyond aaif.io's own blog post distinguishing "Agent Plugins 1.0"
  as independently governed, not an AAIF project, despite being announced
  on the AAIF blog — this apparent tension is unresolved in primary
  sources reached.
- No evidence found that Anthropic acknowledges or targets the Agent
  Plugins 1.0 spec anywhere in Claude Code's own documentation.

## leads:

- Re-check whether Anthropic later joins Agent Plugins 1.0, or whether
  Claude Code's `$schema` field gets a published target — both would
  change the interop picture materially; worth a fresh pass by the 2027-03
  expiry.
- The Cursor `alwaysApply: true`-from-plugin bug (forum threads cited
  above) and the "stuck on stale plugin version" git-ref-pinning bug are
  both live/open as of this research date — re-verify whether Cursor ships
  fixes before treating "rules ship inside a plugin" or "no downgrade
  path" as durable characterizations of the format rather than of the
  current bug state.
- OpenCode's `file://`/git-URL plugin-spec support was confirmed only via
  a GitHub issue and source-code inspection, not prose docs — flag as a
  docs gap if OpenCode is chosen as a target format; verify directly
  against a running `opencode` CLI (none was available in this pass).
- Gemini CLI hooks (`hooks/hooks.json`) and subagents (`agents/*.md`)
  component detail was only confirmed at the manifest-reference level in
  this pass; the dedicated `/docs/hooks/reference` and
  `/docs/core/subagents/` pages were not fetched — worth a follow-up if
  hook/subagent field-level parity with Claude Code becomes a decision
  input.
- Kilo Code and Kiro namespacing/versioning came back undocumented in the
  sources reached in this pass — a deeper, dedicated pass on either would
  be needed before relying on parity claims for them.

## Sources

| Source | Type | Date | Relevance |
|--------|------|------|-----------|
| [code.claude.com/docs/en/plugins-reference](https://code.claude.com/docs/en/plugins-reference) | Docs | 2026-09 | Claude Code `plugin.json` manifest reference |
| [code.claude.com/docs/en/plugins/marketplace-reference](https://code.claude.com/docs/en/plugins/marketplace-reference) | Docs | 2026-09 | Claude Code `marketplace.json` reference |
| [code.claude.com/docs/en/plugins/components](https://code.claude.com/docs/en/plugins/components) | Docs | 2026-09 | Claude Code plugin component layout |
| [code.claude.com/docs/en/plugins/loading#versions-and-updates](https://code.claude.com/docs/en/plugins/loading#versions-and-updates) | Docs | 2026-09 | Claude Code version resolution/update mechanism |
| [code.claude.com/docs/en/plugins/install#choose-an-install-scope](https://code.claude.com/docs/en/plugins/install#choose-an-install-scope) | Docs | 2026-09 | Claude Code install scopes |
| [code.claude.com/docs/en/skills](https://code.claude.com/docs/en/skills) | Docs | 2026-09 | Claude Code's statement that skills follow the Agent Skills open standard |
| [cursor.com/docs/plugins](https://cursor.com/docs/plugins) | Docs | 2026-09 | Cursor plugins overview |
| [cursor.com/docs/reference/plugins](https://cursor.com/docs/reference/plugins) | Docs | 2026-09 | Cursor `plugin.json`/`marketplace.json` field reference |
| [cursor.com/docs/rules](https://cursor.com/docs/rules) | Docs | 2026-09 | Cursor `.mdc` rules frontmatter (alwaysApply etc.) |
| [cursor.com/docs/cli/overview](https://cursor.com/docs/cli/overview) | Docs | 2026-09 | `cursor-agent` update/scope behavior |
| [github.com/cursor/plugins](https://github.com/cursor/plugins/blob/main/README.md) | Repo | 2026-09 | Cursor plugin example repo, marketplace.json examples |
| [github.com/cursor/plugins/issues/136](https://github.com/cursor/plugins/issues/136) | Issue | 2026-09 | Cursor install/enable state undocumented gap |
| [forum.cursor.com — alwaysApply plugin bug](https://forum.cursor.com/t/alwaysapply-true-rules-and-cursorrules-both-silently-treated-as-requestable-instead-of-auto-injected-cursor-3-0-16-macos/157431) | Forum | 2026-09 | Live bug: plugin-shipped always-on rules not injected |
| [forum.cursor.com — stale plugin version pin bug](https://forum.cursor.com/t/add-plugin-github-imports-can-get-stuck-on-stale-plugin-versions/163895) | Forum | 2026-09 | Cursor's real update mechanism is a pinned git ref, not semver |
| [geminicli.com/docs/extensions](https://geminicli.com/docs/extensions/) | Docs | 2026-09 | Gemini CLI extensions overview |
| [geminicli.com/docs/extensions/reference](https://geminicli.com/docs/extensions/reference/) | Docs | 2026-09 | `gemini-extension.json` field reference, `--ref` flag |
| [geminicli.com/docs/extensions/releasing](https://geminicli.com/docs/extensions/releasing/) | Docs | 2026-09 | Update-check mechanism by install type |
| [geminicli.com/docs/extensions/best-practices](https://geminicli.com/docs/extensions/best-practices/) | Docs | 2026-09 | Command namespacing/collision behavior |
| [github.com/google-gemini/gemini-cli/issues/17475](https://github.com/google-gemini/gemini-cli/issues/17475) | Issue | 2026-09 | Proposed (unimplemented) Claude Code plugin import bridge |
| [github.com/autonnel/autonnel-skills/issues/37](https://github.com/autonnel/autonnel-skills/issues/37) | Issue | 2026-09 | Evidence of non-interop: repo mistagged gemini-cli-extension while shipping only `.claude-plugin/` |
| [opencode.ai/docs/plugins](https://opencode.ai/docs/plugins/) | Docs | 2026-09 | OpenCode plugin JS/TS module format |
| [opencode.ai/docs/skills/](https://opencode.ai/docs/skills/) | Docs | 2026-09 | OpenCode SKILL.md multi-path loader incl. `.claude/skills/`, `.agents/skills/` |
| [opencode.ai/docs/agents](https://opencode.ai/docs/agents) | Docs | 2026-09 | OpenCode agent frontmatter |
| [opencode.ai/docs/commands/](https://opencode.ai/docs/commands/) | Docs | 2026-09 | OpenCode command frontmatter |
| [opencode.ai/docs/mcp-servers](https://opencode.ai/docs/mcp-servers) | Docs | 2026-09 | OpenCode MCP server config |
| [github.com/anomalyco/opencode/issues/16669](https://github.com/anomalyco/opencode/issues/16669) | Issue | 2026-09 | Docs gap: undocumented `file://`/`git+` plugin spec forms |
| [raw.githubusercontent.com/sst/opencode — plugin/shared.ts](https://raw.githubusercontent.com/sst/opencode/dev/packages/opencode/src/plugin/shared.ts) | Source | 2026-09 | npm-package-arg-based version resolution (source-level finding) |
| [raw.githubusercontent.com/sst/opencode — config/plugin.ts](https://raw.githubusercontent.com/sst/opencode/dev/packages/opencode/src/config/plugin.ts) | Source | 2026-09 | Local path plugin resolution |
| [docs.factory.ai/harness/plugins](https://docs.factory.ai/harness/plugins) | Docs | 2026-09 | Factory Droid plugin manifest |
| [docs.factory.ai/cli/configuration/plugins](https://docs.factory.ai/cli/configuration/plugins) | Docs | 2026-09 | Factory's explicit `.claude-plugin/` → `.factory-plugin/` translation |
| [github.com/Kilo-Org/kilo-marketplace](https://github.com/Kilo-Org/kilo-marketplace) | Repo | 2026-09 | Kilo Code marketplace component types |
| [kilo.ai/docs/customize/custom-rules](https://kilo.ai/docs/customize/custom-rules) | Docs | 2026-09 | Kilo Code rules convention |
| [kiro.dev/docs/powers/](https://kiro.dev/docs/powers/) | Docs | 2026-09 | Kiro Powers manifest and steering-file inclusion |
| [kiro.dev/blog/powers-supports-plugins/](https://kiro.dev/blog/powers-supports-plugins/) | Blog | 2026-09 | Kiro's adoption of Agent Plugins 1.0-aligned format |
| [agentskills.io](https://agentskills.io) | Standard site | 2026-09 | Agent Skills SKILL.md standard, client adoption showcase |
| [github.com/agentskills/agentskills](https://github.com/agentskills/agentskills) | Repo | 2026-09 | Agent Skills spec governance |
| [aaif.io/blog/from-skills-and-tools-to-portable-agent-plugins](https://aaif.io/blog/from-skills-and-tools-to-portable-agent-plugins) | Blog | 2026-08 | Agent Plugins 1.0 announcement, scope, adopters, non-AAIF governance claim |
| [openai.com/index/agentic-ai-foundation](https://openai.com/index/agentic-ai-foundation/) | Blog | 2026 | AAIF formation, governed projects (MCP, Goose, AGENTS.md) |
| [anthropics/claude-code#50486](https://github.com/anthropics/claude-code/issues/50486) | Issue | 2026-09 | Skill namespacing gap, cited as cross-harness-shared pattern |
| [github.com/vshulcz/deja-vu/issues/3665](https://github.com/vshulcz/deja-vu/issues/3665) | Issue | 2026-09 | Gemini CLI command-collision-rename bug |
