# Research: Harness mods, vendor and third-party distribution channels

## Metadata

**Date:** 2026-11-18
**Domain:** packaging
**Triggered by:** /hex-discuss on shipping "Claude mods" through grimoire. Lane: competitive/vendor. Decision it informs: model a harness-native extension as (A) a kind supported by one harness only, or (B) a harness-neutral kind with one payload per harness. This file reports evidence for and against each and gives no recommendation.
**Expires:** 2027-02-18 (fast-moving: Claude mods, OpenCode V2 and Copilot extensions are all experimental or under 6 months old)

Every URL below was fetched on 2026-11-18 unless marked unverified. Third-party blog claims are marked secondary.

## Direct Answer

No product in this lane has a dedicated, harness-neutral "mod" type. Every first-party channel ships harness-native executable extensions as a component inside that vendor's own plugin or extension package, and every cross-harness tool either translates a small declarative subset, refuses, or gates executable content behind a trust step. The one close precedent for a single-harness executable kind in a third-party manager is Microsoft APM's Copilot-only canvas extension. The one close precedent for shape B is the Agent Plugins 1.0 `extensions` reverse-domain namespace, which carries per-client payloads inside one package but defines no behaviour for them.

## 1. First-party channels

### Claude Code (the mod host)

- **A mod is not a separate package type.** "A mod is a plugin directory" with a required `.claude-plugin/plugin.json` and a required `hooks/hooks.json` whose `modules` key names one JS or TS file exporting `register(on, options)`. "Mods add no required fields" to the manifest. The only mod-specific manifest key is an optional `types` path to a `.d.ts`. Source: https://code.claude.com/docs/en/plugins/mods/reference.md (the Files table), https://code.claude.com/docs/en/plugins/manifest-reference (field `types`).
- **Same detection rule in the components page:** "list a module file under a `modules` key in the same `hooks/hooks.json`. A plugin with one is a mod." Source: https://code.claude.com/docs/en/plugins/components.md (hooks section, line 352 of the fetched copy).
- **What a mod can change:** panes, a band above the prompt, backdrop, spinner, tool-call rows, commands, and tool-call guards. Render sites list per-surface support: Pane runs in Terminal, Desktop and VS Code; Backdrop only on desktop and mobile; some sites terminal and Desktop only. The VS Code extension ignores some element props. Source: https://code.claude.com/docs/en/plugins/mods/reference.md (render-site and element tables). So even inside one harness the payload varies per surface (`e.surface` is `terminal`, `desktop` or `vscode`).
- **Version gating is inside prose, not the manifest.** Entries cover CLI and Desktop "as of v2.1.290" and the VS Code extension "as of v2.1.299"; individual APIs say "Requires Claude Code v2.1.319 or later". The manifest has a general `engines` field (`"claude-code": ">=2.1.400"`, v2.1.323 or later) and older versions reject a manifest that carries it. Source: reference.md above, manifest-reference.
- **Executable flag and trust.** The overview carries a warning: "A mod is code that runs with your permissions. It can read and write your files, start processes, and make network requests." Source: https://code.claude.com/docs/en/plugins/mods/overview.md (Install or update a mod). Mods are on by default since v2.1.286. A built-in guard mod, `sec-default@builtin`, loads ahead of user mods and cannot be turned off by users. Managed settings can limit to organisation mods (`allowManagedModsOnly`), block all hooks (`allowManagedHooksOnly`), or reject `--plugin-dir` (`disableSideloadFlags`). `claude plugin` has a "Review what a mod can do" output listing events handled and calls made (`hooks:` and `calls:` lines). Untrusted directories load no mods until the trust prompt is answered. Source: https://code.claude.com/docs/en/plugins/mods/admin.md.
- **Distribution.** Marketplace entries use `.claude-plugin/marketplace.json` with sources `github`, `git-subdir`, `url`, `archive` (with `sha256` pin), `npm`, and `command`. Sources: https://code.claude.com/docs/en/plugins/create-marketplace.md, https://code.claude.com/docs/en/plugins/marketplace-reference.md (archive `sha256` section: Claude Code "refuses a download that doesn't match").
- **Versioning and updates.** Manifest `version` wins, then entry `version`, then commit SHA (12 chars) for git sources, SHA-256 digest for archives, `unknown` for npm. Pinning a manifest `version` freezes users until the string changes. Auto-update is off by default for third-party marketplaces, on for Anthropic's official ones. Source: https://code.claude.com/docs/en/plugins/loading.md (Versions and updates).
- **Review gate.** Self-hosted marketplaces have none ("no submission form"). Anthropic's directory has a portal submission with a "Prepare for review" step; details live on claude.com and were not fetched (unverified). `claude-plugins-official` takes no portal submissions. Source: https://code.claude.com/docs/en/plugins/publish.md.
- **Signing.** None found for plugins beyond the optional archive `sha256` pin. No signature field in the manifest or marketplace reference (searched the fetched marketplace reference for signature, checksum, delist; only `sha256` and a `flagged-plugins.json` delisting mechanism exist).
- **Declarative versus executable.** A manifest-level split exists in how components are loaded: the loading page restricts project-scope plugins, "Components that run code are restricted further", MCP bundles from files outside the plugin dir are skipped and background monitors do not load for repository plugins. Mods, hooks and monitors are not given a distinct "executable" manifest flag. Source: https://code.claude.com/docs/en/plugins/loading.md (Trust check and component limits).
- **Experimental container.** `experimental.themes` and `experimental.monitors` sit under an `experimental` object "whose manifest shape may still change"; top-level `themes` and `monitors` still load "with a `claude plugin validate` warning". Source: manifest-reference. This is a within-vendor generalization with a compatibility alias.
- **Reserved names.** `claude-`, `anthropic-`, `cc-plugin-` prefixes and `claude-mods` are errors in `claude plugin validate`. Source: manifest-reference (`name`).
- **Cross-harness reach of a plugin.** Anthropic's own directory notes "Some plugin components are Claude Code-only and don't load on claude.ai or in Cowork", with the per-app table on claude.com (not fetched, unverified). Source: publish.md.

### OpenCode (V2 docs)

- **Unit and config.** A plugin is an npm package, a git spec, or a local directory, listed under `plugins` (V2 key) in `opencode.json(c)`. Package specs accept npm names with version, tag or range, and git refs including `::path:` subdirectory selectors. Source: https://opencode.ai/docs/plugins/.
- **One package, several surface payloads.** A package declares parts as `exports`: `.` server, `./tui`, `./gui-experimental`, `./rpc`. "TUI-only plugins are configured separately" in `cli.json` and "A plugin that exports `./gui-experimental` also adds panels, commands, or settings to OpenCode Desktop." Sources: https://opencode.ai/docs/plugins/, https://opencode.ai/docs/build/plugins.
- **Update model.** `opencode plugin check` and `plugin update`; unpinned npm and git plugins are checked in the background without changing the installed package, "Exact npm versions and full Git commit hashes stay pinned". Source: https://opencode.ai/docs/plugins/.
- **Trust.** The fetched pages state no per-plugin consent prompt. A prefix `-` in `plugins` disables an entry, and two built-in policy plugins "ignore removals so that a repository cannot switch off policy enforcement". Review and signing: none stated. Source: https://opencode.ai/docs/plugins/.
- **Generalization break (see section 4).** Plugins in V1 and V2 differ; a V1 migration guide exists (linked from the build page, not fetched).

### Gemini CLI extensions

- **Unit and manifest.** A git repo or GitHub Release archive with `gemini-extension.json` at its root. Declarative fields include `mcpServers`, `contextFileName`, `excludeTools`, `themes` (an array in the manifest), `plan`; directories add `commands/*.toml`, `skills/`, `agents/`, `policies/*.toml`, and `hooks/hooks.json` ("hooks are not defined in the gemini-extension.json manifest"). Source: https://geminicli.com/docs/extensions/reference/.
- **Theme is a first-class manifest field.** This is the only first-party channel found that carries a UI theme as plain JSON data in the manifest. Source: same reference page (Themes section).
- **Versioning and updates.** For a git source "The HEAD commit is always treated as the latest version"; GitHub Releases are checked by the Latest release; channels by branch or tag with `--ref`; `--pre-release` and `--auto-update` exist. Platform-specific archives use `{platform}.{arch}.{name}.{ext}` asset naming with a generic fallback. Source: https://geminicli.com/docs/extensions/releasing/.
- **Gallery and review.** The gallery crawls public repos tagged with the GitHub topic `gemini-cli-extension` daily; "You don't need to submit an issue or email us"; it lists "if it passes validation". No human review gate described. Source: releasing page.
- **Trust.** Install prompts for consent; `--consent` "Acknowledge security risks and skip the confirmation prompt". Extension policies cannot contribute `allow` decisions or yolo mode: "cannot automatically approve tool calls". Extension MCP config drops the `trust` option. Source: reference page (install flags, Policy Engine caution, mcpServers).
- **Signing.** None stated in the fetched pages.

### Cursor

- **Two formats in one product.** "Agent Plugins" (root `plugin.json`, skills and MCP) and "Cursor Plugins" (`.cursor-plugin/plugin.json`, adds rules, agents, commands, hooks, variables). The docs table has an "Available in" column per component: Skills and MCP "Both formats", Rules, Agents, Commands, Hooks "Cursor Plugins". "Cursor detects the format from the plugin manifest". Source: https://cursor.com/docs/plugins.
- **Marketplace.** "Plugins are distributed as Git repositories and submitted through the Cursor team. Every plugin is manually reviewed before it's listed." Team marketplaces (Teams and Enterprise) import GitHub, GitLab, Bitbucket and Azure DevOps repos with install modes Default Off, Default On, Required. Community listings live at cursor.directory, outside Cursor's review. Source: same page.
- **Variable mismatch.** "Cursor does not expand the standard's `${PLUGIN_ROOT}` and `${PLUGIN_DATA}` variables in mcp.json; use `${CURSOR_PLUGIN_ROOT}`." Source: same page. A neutral package needs per-harness rewriting even for portable MCP config.
- **Signing and executable flag.** None stated. A UI-extension surface beyond hooks was not found in this page (Cursor is a VS Code fork; whether VS Code extensions via Open VSX count as its mod channel was not checked, unverified).

### GitHub Copilot (CLI, VS Code, app)

- **Plugin formats.** Agent Plugins 1.0 (root `plugin.json` with the canonical `$schema`) or a legacy CLI format ("a manifest without the Agent Plugins schema uses this legacy format"). In Agent Plugins layout, only skills and MCP use standard locations; "agents, hooks, commands, and LSP servers, are client-specific. Copilot reads these components from the `com.github.copilot` directory. Other clients ignore this directory". Source: https://docs.github.com/en/copilot/concepts/agents/copilot-cli/about-cli-plugins.
- **Extensions are a plugin component.** The component list includes "Extensions: Surface-specific code that adds capabilities such as tools and slash commands", and "A plugin can contain components that only work in a particular surface. Check that the plugin supports the surface you use." Source: same page.
- **CLI extensions.** "a JavaScript module that runs as a separate Node.js process alongside an interactive session"; "currently an experimental feature"; scopes repository, personal, plugin; warning "Extensions for GitHub Copilot CLI execute on your computer with your privileges." No review or approval process described. Source: https://docs.github.com/en/copilot/concepts/agents/copilot-cli/about-cli-extensions.
- **Marketplace.** `marketplace.json` in a git repo or local dir; built-in marketplace is `awesome-copilot`; others must be added; enterprise-managed required and blocked lists; adding a marketplace does not install anything. Source: about-cli-plugins page.
- **Signing.** None stated.

### OpenAI Codex and ChatGPT

- **Plugins.** Parts: skills, MCP servers, browser extensions, hooks ("Plugin hooks are not supported in cloud-orchestrated ChatGPT Work"; "Review and trust plugin hooks before they run"; hook scripts are not deployed by web install). One public universal directory serves ChatGPT and Codex. Source: https://developers.openai.com/codex/plugins.
- **Manifest moved from a vendor file to the neutral one.** "Put OpenAI-specific presentation, registered MCP server mappings, and hook settings under `extensions.com.openai` in root `plugin.json`. Existing `.codex-plugin/plugin.json` files remain supported as a compatibility fallback." Hooks are "manually installed Codex desktop plugins only". Marketplace files are read from `.agents/plugins/marketplace.json` and, as "a legacy-compatible marketplace", `.claude-plugin/marketplace.json`. Hook commands get `PLUGIN_ROOT` and also `CLAUDE_PLUGIN_ROOT` "for compatibility with existing plugin hooks". Source: https://developers.openai.com/codex/plugins/build.
- **Review gate.** The build docs carry a "Submit a Claude Code plugin" section in navigation (title only; content not read, unverified).

### VS Code Marketplace (mature reference)

- **Per-target payloads inside one extension id.** `engines.vscode` gates API compatibility; platform-specific VSIX packages per target via `--target` with a no-flag fallback; pre-release channel; `extensionKind` and `extensionPack` in the manifest. Sources: https://code.visualstudio.com/api/working-with-extensions/publishing-extension, https://code.visualstudio.com/api/references/extension-manifest.
- **Trust.** The Marketplace "signs all extensions when they're published" and VS Code checks the signature at install; a malware scan including a "clean room VM" precedes publication; verified-publisher badge via domain proof; a publisher trust dialog since VS Code 1.97; and "The extension host has the same permissions as VS Code itself." Source: https://code.visualstudio.com/docs/configure/extensions/extension-runtime-security. This is the only channel in the lane with signature verification at install.

## 2. Third-party managers and aggregators

### `npx skills` and skills.sh (Vercel)

- **Scope.** Skills only (`SKILL.md`), installed into 75+ agents' skill dirs. About 39.9k GitHub stars, last push 2026-11-17 (https://api.github.com/repos/vercel-labs/skills). The README says "Supports OpenCode, Claude Code, Codex, Cursor, and 75 more". Source: https://github.com/vercel-labs/skills.
- **Executable extensions: none, no exclusion stated.** skills.sh docs contain "no statement excluding executable or harness-specific content" and do not mention hooks. They state "routine security audits that assess skills and their contents for malicious content" and disclaim guarantees. Source: https://skills.sh/docs.
- **Reads Claude plugin manifests only to find skills.** "If `.claude-plugin/marketplace.json` or `.claude-plugin/plugin.json` exists, skills declared in those files are also discovered." A mod's `hooks/hooks.json` is not consulted. Source: https://github.com/vercel-labs/skills/blob/main/docs/creating-skills.md.
- **Where "works with X" lives.** A compatibility table of agents versus features (`allowed-tools`, `context: fork`, hooks) in `docs/agents.md`, outside the package. The table shows Hooks "No" for most agents and "Yes" for some; installs are not blocked on it. Source: https://github.com/vercel-labs/skills/blob/main/docs/agents.md.
- **Integrity.** `skills-lock.json` v2 with `computedHash` per skill. Source: README.

### rulesync (about 1.6k stars)

- **Translates, does not ship native bytes.** One canonical source tree (`.rulesync/`) generates per-tool files. Feature matrix tool by feature: rules, ignore, mcp, commands, subagents, skills, hooks, permissions, checks. Source: https://github.com/dyoshikawa/rulesync (README matrix).
- **Hooks are translated and per-tool overridable.** `.rulesync/hooks.jsonc` uses canonical camelCase events "and Rulesync translates them per tool"; tool-named sections (`cursor`, `claudecode`, `opencode`) carry single-harness hooks. For OpenCode and Kilo "hooks are emitted as JavaScript plugins (`.opencode/plugins/rulesync-hoo...`)". Source: https://github.com/dyoshikawa/rulesync/blob/main/docs/reference/file-formats.md (hooks section).
- **No themes, panes, bands or status lines.** The only status-line-like hit is a pass-through: "Any other key is a top-level `settings.json` key" in the `claudecode` block. Source: same file (Claude Code settings note).
- **Plugin packaging targets.** `claudecode-plugin`, `antigravity-plugin`, `augmentcode-plugin`, `zcode-plugin`, `vibe-plugin`, `devin-plugin`, `kimi-code-plugin` write components into an existing plugin dir; excluded from `--targets "*"`; rulesync "preserves plugin manifests, marketplace metadata, scripts, and other non-component assets". Source: README and https://github.com/dyoshikawa/rulesync/blob/main/docs/guide/plugin-packaging.md.

### ruler (about 3.0k stars)

- Distributes rules, MCP config, skills and subagents across about 25 agents; its README matrix has columns for rules file, MCP config, skills location and subagents location and no hooks or UI column. Source: https://github.com/intellectronica/ruler (README table). No exclusion statement found for executable extensions.

### Smithery

- MCP server registry plus a skills registry; the CLI delegates skills to the upstream installer ("`npx skills add <skill>`"). No harness-specific executable extension type. Source: https://github.com/smithery-ai/cli (README).

### Microsoft APM (4.4k stars, v0.36.1 published 2026-11-18)

- **Closest analogue to this question.** One `apm.yml`, lockfile `apm.lock.yaml`, targets copilot, claude, cursor, opencode, codex, gemini and more. Primitive kinds: "instructions, skills, prompts, agents, hooks, commands, plugins, MCP servers, and experimental canvas extensions. Each kind has its own integrator that knows how to deploy it into each harness." Sources: https://github.com/microsoft/APM, https://microsoft.github.io/apm/llms-full.txt (line 288 of the fetched copy; fetch 2026-11-18).
- **"Works with harness X" lives in a primitive-by-target matrix** with cell values `native`, `compiled`, `unsupported`, `gated`. Legend: "`unsupported` APM does not deliver this primitive to this harness." Hooks are native for several targets and unsupported for others (OpenCode among them in the fetched matrix). Source: same file (Primitives and targets).
- **Single-harness executable kind exists as precedent.** "Canvas extensions (experimental): GitHub Copilot CLI canvas extensions: a directory bundle whose entry file is `extension.mjs` (executable Node.js). Copilot-only. Behind the `canvas` experimental flag." The canvas row is `gated` for Copilot and `unsupported` for every other target. With the flag off, ".apm/extensions/ is ignored entirely." The package carries it under `.apm/extensions/<name>/` and APM deploys it verbatim to `.github/extensions/<name>/`. Source: same file, plus https://microsoft.github.io/apm/integrations/canvas/ (section text read from the llms-full copy).
- **Executable trust gate.** "APM blocks unapproved executable primitives from dependency packages: hooks, `bin/` executables, self-defined MCP servers, LSP servers, and canvas extensions. The gate is opt-in for backward compatibility; a project `executables:` block or org policy enables it. Text primitives (skills, agents, instructions) are never gated." Approval via `apm approve <pkg>`, keyed `name#version@sha256:<digest>` for bundles. Source: same file (executable trust, bundle install).
- **Integrity.** `content_hash` (SHA-256 of the package file tree), `deployed_file_hashes`, `apm audit --ci`; every install scans for hidden Unicode and "Critical findings block the install". Source: same file.
- **Claude plugins.** `plugin.json` packages are "normalized at install into the same primitives" (`plugins / * = compiled`). Whether a plugin's `hooks.json` `modules` entry (a Claude mod) survives that normalization was not found in the fetched text (unverified). No grep hit for "mods" or "status line".

### Other 2026 entrants (secondary only, not fetched)

- Search results name ECC v2.0.0 (cross-harness agents, skills, rules across seven harnesses), HarnessKit (Rust manager of skills, MCP, plugins, hooks across agents), x-cmd's agents catalog, and wshobson/agents ("one plugins/ directory, six harness adapters"). None was opened; all unverified. Source: web search 2026-11-18 (result titles from augmentcode.com, awesome.ecosyste.ms, niteagent.com).

### Agent Plugins 1.0 spec (cross-vendor)

- **Status.** v1.0.0, vendor-neutral; the initial steering committee has maintainers from Amazon, Cursor, Microsoft, OpenAI and Vercel. Source: https://agent-plugins.org.
- **Closed core, open namespaces.** Component types in v1: exactly two, skills and MCP servers; "Other component types are outside the v1 format". Manifest schema is closed (`$schema`, `name`, `version`, `description`, `author`, `homepage`, `repository`, `license`, `keywords`, `extensions`). Source: https://agent-plugins.org/specification (sections 5, 7).
- **Per-client payload rule.** "Client-specific manifest data MUST be represented under a reverse-domain namespace in `extensions`." "Client-specific files MUST be represented under a top-level directory named for that namespace." "A client MUST ignore manifest entries for namespaces it does not implement without validating the contents of their values." "Agent Plugins assigns no portable discovery, validation, loading, or failure semantics to client extension data or files." Lack of support for "a client extension is not itself an error". Source: same specification (section 8, section 11).
- **Sandboxing.** "They do not sandbox a plugin subprocess or restrict paths supplied at runtime." Source: same specification (section 4.1).
- **Observed client behaviour (secondary).** VS Code "currently ignores client extension data and directories in Agent Plugins 1.0 packages", i.e. silent drop. Source: https://www.digitalapplied.com/blog/agent-plugins-1-0-ga-what-still-does-not-port (published 2026-08-16; secondary, quotes VS Code docs not independently fetched). An earlier verification of the spec exists in `.agents/research/research_agent_plugins_spec_verify.md` (2026-09-27).

## 3. Deliberate differences between products

| Question | Positions found |
|---|---|
| One package for many harnesses vs one per harness | One neutral package: Agent Plugins, Cursor (accepts both), Copilot (new plugins should use it), Codex (new packages should use root `plugin.json`), APM manifest, skills CLI (skills only). One package per harness: Claude plugins and mods, Gemini extensions, OpenCode npm packages, Copilot legacy format and canvases. |
| Translate vs ship native bytes | Translate: rulesync (canonical hooks to per-tool files, OpenCode hooks as generated JS plugins), APM `compiled` cells (prompts to Claude commands). Native bytes: APM canvas (verbatim copy), APM bundles ("Bundle files are copied, not re-rendered"), Agent Plugins namespaces, Claude marketplace `archive` source. |
| Where "works with X" lives | Package type (Gemini extension, Claude plugin); a manifest key (`engines.vscode`, Claude `engines.claude-code`); a directory name (`com.github.copilot/`, `com.example.client/`, `.apm/extensions/`, Gemini `{platform}.{arch}` asset names); a manifest namespace (`extensions.com.openai`); an external matrix not in the package (skills `docs/agents.md`, APM target matrix, rulesync tool matrix); a docs column ("Available in" in Cursor). |
| Unsupported on harness Y | Silent ignore by spec (Agent Plugins, section 11); silent drop in VS Code (secondary); explicit `unsupported` and not delivered (APM); opt-in experimental flag with item ignored when off (APM canvas); skip with one warning per file (APM bundles); install then partial reach noted in docs (Claude directory component table, not fetched). No product in this lane refuses a whole package because one component is unsupported on the active harness. |
| Executable vs declarative | Explicit gate: APM (text never gated; hooks, `bin/`, MCP, LSP, canvas gated, `apm approve`); Claude (mods get a built-in guard plus admin review output, but no manifest flag distinguishes mod from skill); VS Code (signature plus malware scan, no sandbox); Copilot and Codex (doc warnings and trust-before-run for hooks); skills.sh (audits skills, no executable class). |
| Review and signing | Manual review: Cursor Marketplace. Automated crawl and validation: Gemini gallery. Signature verified at install: VS Code only. Hash pin: Claude `archive` sha256, APM lock hashes, skills lock hashes. None found: OpenCode, Copilot, Codex (page text), Gemini signing. |

## 4. Generalization and reversal cases

- **Codex: vendor manifest to neutral manifest.** `.codex-plugin/plugin.json` was the original; root `plugin.json` with `extensions.com.openai` is now the "portable entry point" and the old file "remain[s] supported as a compatibility fallback". Hooks stayed vendor-specific and moved under the namespace. Hooks reach only "manually installed Codex desktop plugins". Source: https://developers.openai.com/codex/plugins/build.
- **Copilot: legacy to Agent Plugins.** The CLI keeps the legacy format and decides the format by the `$schema` key; legacy retains "configurable component paths"; the new format fixes locations and moves agents, hooks, commands and LSP into `com.github.copilot/`. Source: about-cli-plugins page.
- **Cursor: two formats side by side.** Format detection from the manifest; the neutral format lacks rules, agents, commands, hooks; variable names differ. Source: https://cursor.com/docs/plugins.
- **Claude marketplace format as a de facto neutral container.** Codex reads `.claude-plugin/marketplace.json` as "legacy-compatible"; `npx skills` reads `.claude-plugin/` manifests for skills; Codex hooks also export `CLAUDE_PLUGIN_ROOT`. Sources: Codex build page, skills `creating-skills.md`. The reverse direction (a harness-native extension kind becoming neutral) was not found.
- **OpenCode V1 to V2 broke a translated executable.** rulesync had to split a target: `opencode` writes V2, `opencodev1` writes the V1 shapes including "the V1 `RulesyncHooksPlugin` plugin module, which OpenCode 2.x does not run"; both write the same paths, so `opencodev1` is excluded from `targets: ["*"]`. A second note says OpenCode 2.x reads each agent file in one format only since v2.1.4. Source: https://github.com/dyoshikawa/rulesync/blob/main/docs/reference/file-formats.md (OpenCode V1 note, OpenCode V2 agent note). This is the one found case of a translated executable going stale for existing packages after a host's plugin API changed.
- **Claude experimental to stable shape.** `themes` and `monitors` moved from top level to `experimental`, with the top-level key still loading plus a validate warning. Source: manifest-reference. Existing packages keep working; the migration cost is a warning.
- **Single-vendor kind added by a neutral manager.** APM added Copilot's canvas as an experimental primitive row that is `unsupported` everywhere else, without changing existing rows. Source: APM llms-full (fetched 2026-11-18).

## Evidence for and against each shape (no recommendation)

### (A) A kind supported by one harness only

For:
- Every first-party channel already treats the mod as a component of its own plugin type, not a neutral type (Claude `hooks.json` `modules`, Gemini manifest `themes`, Copilot "Extensions" component, OpenCode `./tui` and `./gui-experimental` exports).
- APM's canvas is a working example: `gated` on one harness, `unsupported` on all others, flag-off means ignored, verbatim deploy, executable trust gate. No translation layer.
- Agent Plugins assigns "no portable discovery, validation, loading, or failure semantics" to client extensions, so a neutral abstraction has no standard to lean on.
- rulesync carries no themes, panes or status lines at all, and its OpenCode hook translation broke on a host version change, which argues against promising translation for UI-level surfaces.

Against:
- Claude mods have no separate manifest marker: a mod is a plugin whose `hooks.json` carries `modules`. A grimoire kind would have to detect or declare that itself, and a Claude plugin may mix mod and non-mod components (skills, agents) that other grimoire kinds already cover.
- Mod surface support varies inside Claude (terminal, Desktop, VS Code, mobile), so "one harness" still has per-surface payload variance.
- Per-harness kinds multiply: Gemini, Copilot and OpenCode each have their own executable extension component, so a one-harness kind per vendor repeats the same lifecycle work.

### (B) A harness-neutral kind with a payload per harness

For:
- The Agent Plugins `extensions` namespace and per-namespace top-level directory is a ratified mechanism, with a rule for unknown namespaces (ignore without validating). Codex and Copilot already use it for hooks and other client-specific components.
- OpenCode packages already hold several payloads (`.`, `./tui`, `./gui-experimental`) in one npm package, VS Code holds one VSIX per platform under one id, Gemini holds one archive per platform, so per-target payloads under one identity are routine.
- A neutral identity lets a single name and version cover a theme that Gemini expresses as manifest JSON and Claude expresses as a mod or an `experimental.themes` file.

Against:
- The standard's silence on semantics means each client may drop the payload quietly (VS Code, secondary source), so "installed" does not imply "active".
- The neutral manifest is closed and limited to skills and MCP; Cursor, Copilot and Codex each add vendor-specific manifest paths and variable names (`${CURSOR_PLUGIN_ROOT}`), so even the neutral core needs per-harness rewriting.
- No product found models a UI or interface extension (pane, band, status line, toast) as neutral content. The neutral side of every comparison is skills and MCP.
- Version coupling: Claude APIs are gated by "Requires Claude Code v2.1.x" per method, so a neutral package version does not express per-harness compatibility without extra per-payload metadata (cf. `engines.vscode`, Claude `engines.claude-code`).

## negative:

- No third-party manager (skills CLI, rulesync, ruler, Smithery) distributes a harness UI mod, and none states why. skills.sh and ruler have no exclusion statement; rulesync simply has no surface for it.
- No product found with signature verification for harness extensions except the VS Code Marketplace. Claude, Gemini, OpenCode, Copilot and Codex fetched pages state none.
- Anthropic directory review details (claude.com pages) and the Codex "Submit a Claude Code plugin" content were not fetched. Whether mods are admitted to Anthropic's directory is unverified.
- APM handling of a Claude plugin whose `hooks.json` has `modules` is unverified.
- Cursor's VS Code-extension channel (Open VSX) was not examined; ECC, HarnessKit, x-cmd and wshobson/agents were only seen in search snippets.
- The digitalapplied article on VS Code dropping client extensions is secondary; the primary VS Code page was not fetched.
- The OpenCode V1 migration guide and OpenCode `./tui` docs were not read; trust prompts for OpenCode plugins were not found, which is absence of evidence only.
- Version numbers in APM third-party blogs conflict (v0.8.11 vs v0.25.0 vs the GitHub release v0.36.1). The release API figure is the one used here.

## leads:

- Claude mod runtime and trust, deeper: whether `claude plugin` review output and `sec-default` guard could be reproduced or consumed by grim at install time (mods admin page, 45 KB, read only in part).
- APM source dive: how `executables`, `apm approve` and the target matrix are encoded, since this is the closest model for a gated single-harness kind.
- Anthropic directory review rules (claude.com/docs/directory/publish): do mods pass review, and is a mod flagged differently from other plugin components.
- Agent Plugins GitHub issues and TSC notes: any proposal to standardize hooks or UI components, which would change the (B) analysis.
- OpenCode V2 TUI and GUI plugin docs plus Copilot canvas docs: which surfaces are the true peers of Claude mods, to size how many single-harness kinds (A) would need.
- Gemini and Copilot extension gallery validation rules: what automated checks exist for executable content.
- Cursor and VS Code extension (Open VSX) channel: Cursor's real equivalent of a mod may be a VS Code extension, which has signing and per-platform targets.

## Sources

| Source | Type | Fetched | Relevance |
|---|---|---|---|
| https://code.claude.com/docs/en/plugins/mods/reference.md | Vendor docs | 2026-11-18 | Mod file layout, render sites, surfaces, settings |
| https://code.claude.com/docs/en/plugins/mods/overview.md | Vendor docs | 2026-11-18 | Mod definition, trust warning |
| https://code.claude.com/docs/en/plugins/mods/admin.md | Vendor docs | 2026-11-18 | Built-in guard, managed policy, review output |
| https://code.claude.com/docs/en/plugins/manifest-reference | Vendor docs | 2026-11-18 | plugin.json fields, experimental container, reserved names |
| https://code.claude.com/docs/en/plugins/components.md | Vendor docs | 2026-11-18 | Mod detection rule |
| https://code.claude.com/docs/en/plugins/loading.md | Vendor docs | 2026-11-18 | Version resolution, update model, repo-plugin component limits |
| https://code.claude.com/docs/en/plugins/publish.md | Vendor docs | 2026-11-18 | Distribution routes, directory submission |
| https://code.claude.com/docs/en/plugins/create-marketplace.md | Vendor docs | 2026-11-18 | Marketplace and source types |
| https://code.claude.com/docs/en/plugins/marketplace-reference.md | Vendor docs | 2026-11-18 | archive sha256, strict, sources |
| https://opencode.ai/docs/plugins/ and https://opencode.ai/docs/build/plugins | Vendor docs | 2026-11-18 | OpenCode V2 plugins, exports parts |
| https://geminicli.com/docs/extensions/reference/ and /releasing/ | Vendor docs | 2026-11-18 | Gemini manifest, gallery, releases |
| https://cursor.com/docs/plugins | Vendor docs | 2026-11-18 | Two formats, marketplace review |
| https://docs.github.com/en/copilot/concepts/agents/copilot-cli/about-cli-plugins | Vendor docs | 2026-11-18 | Copilot plugin formats, com.github.copilot |
| https://docs.github.com/en/copilot/concepts/agents/copilot-cli/about-cli-extensions | Vendor docs | 2026-11-18 | CLI extensions, experimental, privilege warning |
| https://developers.openai.com/codex/plugins and /build | Vendor docs | 2026-11-18 | Codex plugins, extensions.com.openai, hooks |
| https://code.visualstudio.com/api/working-with-extensions/publishing-extension | Vendor docs | 2026-11-18 | engines, platform targets, pre-release |
| https://code.visualstudio.com/docs/configure/extensions/extension-runtime-security | Vendor docs | 2026-11-18 | Signing, malware scan, trust dialog |
| https://agent-plugins.org and /specification | Open spec | 2026-11-18 | extensions namespace rules, closed schema |
| https://github.com/vercel-labs/skills (README, docs/creating-skills.md, docs/agents.md) and https://skills.sh/docs | Repo, docs | 2026-11-18 | Skills CLI scope, plugin manifest discovery, audits |
| https://github.com/dyoshikawa/rulesync (README, docs/reference/file-formats.md, docs/guide/plugin-packaging.md) | Repo | 2026-11-18 | Translation, hooks, plugin targets, OpenCode V1 split |
| https://github.com/intellectronica/ruler | Repo | 2026-11-18 | Feature columns, no hooks or UI |
| https://github.com/smithery-ai/cli | Repo | 2026-11-18 | MCP and skills registry scope |
| https://github.com/microsoft/APM and https://microsoft.github.io/apm/llms-full.txt | Repo, docs | 2026-11-18 | Target matrix, canvas, executable gate |
| https://www.digitalapplied.com/blog/agent-plugins-1-0-ga-what-still-does-not-port | Blog (secondary) | 2026-11-18 | Client behaviour on foreign extension data |
