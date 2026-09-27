# Research: Plugin format definitions and cross-harness interoperability (Claude Code, GitHub Copilot, OpenAI Codex)

<!--
Technology-landscape research. Filename and location: this project's
documented research convention (`.agents/research/research_[topic].md`).
Owner: a researcher worker. Handoff to: /hex-architect, /hex-plan.
-->

## Metadata

**Date:** 2026-09-27
**Domain:** packaging
**Triggered by:** /hex-discuss harness-native-marketplaces — plugin format interop check
**Expires:** 2027-03-27

## Direct Answer

All three harnesses now ship a real "plugin" concept (manifest + marketplace
+ multi-component bundle), and as of September 2026 there is a genuine
cross-vendor standard — **Agent Plugins 1.0** (agent-plugins.org, spec at
[github.com/agentplugins/agent-plugins-spec](https://github.com/agentplugins/agent-plugins-spec)),
GA'd 2026-08-06, backed by OpenAI, Microsoft, AWS, Anysphere (Cursor),
Vercel, with Google as a listed core maintainer. **GitHub Copilot (VS Code,
CLI, app) and OpenAI Codex (CLI, app, IDE) both implement it. Claude Code
does not.** The standard itself is narrow: it standardizes only `plugin.json`
(root, unqualified) + `skills/*/SKILL.md` + `mcp.json`. Everything else —
agents/subagents, slash commands, hooks, LSP servers, and any always-on
rules/instructions file — is explicitly left vendor-specific, parked under a
reverse-DNS `extensions.<vendor>` manifest key and a matching
`<vendor-namespace>/` directory (e.g. `com.github.copilot/`, `com.openai/`).

None of the three harnesses lets an **always-on** rules/instructions file
(CLAUDE.md / AGENTS.md / copilot-instructions.md equivalent) ship as a
first-class, auto-loaded plugin component under the portable standard.
Claude Code explicitly refuses to load a `CLAUDE.md` at a plugin root as
context (`claude plugin validate` warns on it) — the documented workaround
is to put the instructions inside a skill. Copilot's own manifest reference
lists a `com.github.copilot/rules/` directory as a Copilot-specific
(non-portable) component, so Copilot plugins *can* carry rules, but only
as a Copilot-native extension, invisible to any other client. Codex's
plugin docs make no mention of a rules/instructions component in a plugin
at all — AGENTS.md is a separate, non-plugin repo/user-scope mechanism.

**Would an unmodified Claude Code plugin directory install and load
elsewhere?** VS Code's Agent Plugins host explicitly auto-detects
`.claude-plugin/plugin.json` as one of four recognized manifest shapes
(Agent Plugins 1.0, Copilot legacy, Claude, OpenPlugin legacy) and
documents `${CLAUDE_PLUGIN_ROOT}` substitution for MCP fields — so **VS
Code is the one non-Claude client with documented, if only partially
specified, awareness of the Claude format**. Its docs stop short of
confirming full component parity (hooks, `agents/`, `commands/`,
`output-styles/` behavior under the Claude branch is undocumented) or of
reading a Claude `marketplace.json` as a marketplace source. Copilot CLI
and Codex CLI/App have no documented cross-read of `.claude-plugin/` at
all — going the other direction, Claude Code has no documented cross-read
of Agent Plugins 1.0's `plugin.json` (Claude's own `plugin.json` at the
same path with different semantics would very likely misparse or silently
strip the unrecognized `$schema`/`extensions` keys; not directly tested).

## Per-Harness Comparison

| | **Claude Code** | **GitHub Copilot** (VS Code / CLI / app) | **OpenAI Codex** (CLI / App / IDE) |
|---|---|---|---|
| **Manifest file** | `.claude-plugin/plugin.json` at plugin root; optional (components load from standard layout even without it) | `plugin.json` at plugin root, required for both formats. Agent Plugins 1.0 is selected by `"$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json"`; without that schema string, defaults to legacy Copilot format | `plugin.json` at plugin root under Agent Plugins 1.0 (same `$schema` string); a `.codex-plugin/plugin.json` legacy/compat path also documented as still supported |
| **Required fields** | Only `name` (kebab-case, no spaces/`@`/`:`/path separators) | Agent Plugins 1.0: `$schema` + `name` (1–64 chars, lowercase ASCII+digits+hyphens+periods, no `--`/`..`). Legacy: `name` (kebab-case, ≤64 chars) | `name`, `version`, `description` treated as "core portable" identity fields per third-party technical summary of the OpenAI plugin-directory docs; primary docs don't spell out a strict required/optional table as explicitly as Claude Code's |
| **Published JSON Schema** | `$schema` field exists in `plugin.json` for editor autocomplete, but Claude Code "ignores it at load time" — no canonical published schema URL for `plugin.json` itself was found (only a `claude-code-settings.json` schema on schemastore.org, a different file) | Yes — `https://agent-plugins.org/schemas/1.0.0/plugin.schema.json` (and a companion `mcp.schema.json`) is the actual conformance schema, vendor-neutral | Same Agent Plugins 1.0 schema URL when the plugin opts into the portable format |
| **Marketplace/registry file** | `.claude-plugin/marketplace.json` at a marketplace repo/dir root. Top-level: `name`, `owner`, `description`, `plugins[]`. Each `plugins[]` entry: `name`, `source`, `description`, optional `displayName`, `defaultEnabled`, `strict`, `version`, and any `plugin.json` field | `marketplace.json`, required and sufficient file for a marketplace. Documented top-level fields (per GitHub Docs, partially recovered): `name`, `owner` (`name`/`email`), `metadata` (`description`, `version`); per-entry: `name`, `description`, `version`, `source` (relative path in the examples fetched — a "full field set" is referenced in the CLI reference for additional source types but not itself enumerated in what was retrieved) | Local/dev marketplaces at `~/.agents/plugins/marketplace.json` (personal) or `.agents/plugins/marketplace.json` (repo-scoped); public plugins go through a submission portal into "the universal plugin directory shared by ChatGPT and Codex" rather than a federated marketplace.json a user points at directly |
| **Source types** | Relative path, `github`, `git-subdir`, `git` (marketplace-source only), `url` (git URL, or direct `marketplace.json` link as a marketplace source), `archive` (zip over HTTPS + optional `sha256` pin), `npm`, `command` (locally-generated dir) | Not fully enumerated in the pages retrieved; CLI plugin reference confirms `ref` (branch/tag) and `sha` (full 40-char commit) as the two precision levels for git-hosted plugin sources | Not enumerated in primary docs surfaced; local-path and (implicitly) the submission-portal path are documented, no explicit git/npm/archive source-type table found |
| **Component types carried** | Skills (`skills/*/SKILL.md`), commands (`commands/*.md`, legacy/back-compat — "prefer skills/ for new plugins"), agents (`agents/*.md`), hooks (`hooks/hooks.json`), MCP servers (`.mcp.json` + inline + `.mcpb`/`.dxt` bundles), LSP servers (`.lsp.json`), output styles, workflows (`.js`), themes, monitors (experimental), executables (`bin/`), default settings (`settings.json`, only `agent`/`subagentStatusLine` keys). **No rules/CLAUDE.md component** — explicitly rejected at plugin root | Portable (Agent Plugins 1.0 spec): skills (`skills/*/SKILL.md`), MCP servers (`mcp.json`). Copilot-specific, namespaced under `com.github.copilot/`: custom agents (`*.agent.md` in `agents/`), slash commands (`commands/`), hooks (`hooks/hooks.json`), LSP servers (`lsp.json`), **and `rules/`** — i.e. Copilot's plugin format *does* carry an always-on-instructions-like component, but only in its own non-portable namespace | Portable: skills (`skills/*/SKILL.md`), MCP servers (`mcp.json`). OpenAI-specific, under `extensions.com.openai`: `interface` (branding/UI), `apps` (MCP-to-UI mappings via `.app.json`), `hooks` (lifecycle). No documented rules/instructions component inside a plugin; AGENTS.md is a separate mechanism scoped to repo root / nested `AGENTS.override.md`, not a plugin artifact |
| **Namespacing of installed components** | Every component namespaced under the plugin's manifest `name`: e.g. skill/command `hello` in plugin `my-plugin` runs as `/my-plugin:hello`. Entry name (marketplace) and manifest name can differ; components always namespace under the manifest name | Not directly documented in pages retrieved for command/skill invocation syntax; Copilot-specific components live under the `com.github.copilot/` directory, which is the isolation mechanism rather than a `plugin:component` invocation string | Not documented in pages retrieved |
| **Versioning/pinning/update** | See dedicated section below | See dedicated section below | See dedicated section below |
| **Cross-reads another vendor's format** | No documented reading of Agent Plugins 1.0 `plugin.json`/`mcp.json`, or of Copilot/Codex marketplace files | VS Code: **yes, partially** — auto-detects `.claude-plugin/plugin.json` as a recognized manifest shape alongside Agent Plugins 1.0, legacy Copilot (`plugin.json`), and legacy OpenPlugin (`.plugin/plugin.json`); documents `${CLAUDE_PLUGIN_ROOT}` substitution for MCP fields under the Claude branch. Copilot CLI / Copilot app: no documented Claude-format awareness found | No documented reading of Claude's `.claude-plugin/` or Copilot's `com.github.copilot/` namespace |
| **Would an unmodified Claude-format plugin install & load all components elsewhere?** | N/A (native) | **Partial, undocumented in full** — VS Code recognizes the manifest and at least MCP-field substitution; whether VS Code's Claude branch also loads Claude's `agents/`, `commands/`, `hooks/hooks.json`, `output-styles/`, and namespaces them the way `com.github.copilot/*` is namespaced is not stated in the fetched docs. No claim that VS Code reads a Claude `marketplace.json` as a marketplace source | **No** — no documented format detection or cross-read of `.claude-plugin/` at all |
| **Install scopes** | User (`~/.claude/settings.json`), project (`.claude/settings.json`, committed), local (`.claude/settings.local.json`, gitignored), managed/org (enterprise `managed-settings.json` / MDM / server-managed settings — force-enable or force-block, highest precedence), plus a distinct "synced from claude.ai account" origin | Documented as user-level (`~/.copilot/settings.json`), repo-level (`.github/copilot/settings.json`), and org/enterprise ("Enterprise-managed plugins in GitHub Copilot CLI" reached public preview 2026-05-06 per GitHub changelog) — VS Code additionally documents "user" and "workspace" scopes for its own plugin panel | Documented: workspace-level install with per-role "Available"/"Installed" policy and workspace-admin-managed marketplace sync; user/project/org three-way split as clean as Claude Code's is **not** confirmed in the pages fetched |
| **Enable/disable granularity** | Whole-plugin only, via `enabledPlugins` keyed `<name>@<origin>: true|false`, mergeable across 6 precedence tiers (`--add-dir` < user < project < local < flag < managed) | Whole-plugin only in every page fetched; no per-component (e.g. "load skill but not hooks") toggle documented | Whole-plugin only, no per-component granularity documented |
| **Org-managed marketplace controls** | Extensive: `extraKnownMarketplaces` (require), `enabledPlugins` (force on/off), `strictKnownMarketplaces`/`blockedMarketplaces` (allow/deny by source, incl. `hostPattern`/`pathPattern`/owner-wildcard matching), `disableSideloadFlags`, `disableCommandPluginSources`, `strictPluginOnlyCustomization`, seed dirs for offline CI (`CLAUDE_CODE_PLUGIN_SEED_DIR`), release channels via parallel marketplaces | Enterprise-managed plugins (public preview since 2026-05-06) with per-role install policy; allow/deny-list mechanics not confirmed to the same field-level detail in what was fetched | Workspace admin can restrict marketplace sync and control availability per role; allowlist/blocklist mechanism not confirmed in primary docs fetched |

## Version semantics

### Claude Code

- **Comparison method: opaque string equality, not semver precedence.** Claude Code "computes a version" for every installed plugin and stores it in `installed_plugins.json`; an update runs only when the *newly computed* string differs from the stored one. There is no semver parser in this path.
- **`version` is explicitly not validated as semver.** The manifest reference states outright: "A version string, not checked against semver."
- **Resolution order** (per source type other than `command`): manifest `plugin.json` `version` field → marketplace-entry `version` field → source-type-derived fallback (commit SHA, archive SHA-256 digest, or `unknown` for local/npm sources with no git backing).
- **Build-metadata-only bump (`1.2.0+abc` → `1.2.0+def`) IS detected as an update.** Since comparison is plain string inequality (not semver, which per spec section 10 treats build metadata as insignificant for precedence), any literal string change — including build-metadata-only — is a different computed version, triggers a new `cache/<marketplace>/<plugin>/<version>/` directory, and is reported as an available update. Conversely, if the author does *not* change the `version` string, Claude Code will not detect a real content change either (documented failure mode: "author pushed new commits... version Claude Code computes is unchanged, so nothing changes on disk" — only fixed by leaving `version` unset so the commit SHA becomes the version).
- **Can a marketplace entry list more than one version at once?** No — a `plugins[]` entry names exactly one plugin at exactly one `source`; there is no array-of-versions shape documented. Multiple *installed* version directories can coexist on disk (`cache/<marketplace>/<plugin>/<version>/`, old ones garbage-collected 14 days after becoming orphaned), but that is a caching artifact of successive updates, not a marketplace feature for choosing among versions.
- **Can a user install an older version other than by pinning a git ref/SHA?** No documented `--version` flag on `claude plugin install`/`update`; `plugin update` only ever moves to "the latest version its marketplace offers." The only way to land on an older version is a `ref`/`sha` pin in the marketplace entry's `source` (git-based sources), a `sha256` pin (`archive` source), or a pinned `version` string in the manifest/entry that the author has since bumped upstream (i.e., pin-by-not-updating).

### GitHub Copilot

- Primary docs fetched (`docs.github.com` plugin-creating, plugins-marketplace, plugins-finding-installing) **do not document** the comparison algorithm (string vs. semver), do not state whether `version` is validated as semver, and do not discuss multi-version listings — **undocumented** in what was retrieved.
- What is documented (CLI plugin reference): `github`, `url`, and `git-subdir` sources share `ref` (branch/tag) and `sha` (full 40-char commit) fields, "the two precision levels" for pinning — mirroring Claude Code's own `ref`/`sha` pair almost verbatim.
- Secondary/community evidence (GitHub issue discussion, not a primary vendor doc, flagged as such) states plainly that "`version` in `plugin.json` is informational — there is no semver resolver and you cannot pin a version at install time," and that the git ref/SHA in the marketplace entry is "the real version mechanism." This is consistent with, but not confirmed word-for-word by, the primary pages fetched.
- **Installing an older version other than via ref/SHA pin:** no install-time version flag documented in the primary Copilot pages retrieved — undocumented/not found; the community source above says explicitly "cannot pin a version at install time" outside of a ref/SHA.

### OpenAI Codex

- Primary docs fetched (`learn.chatgpt.com/docs/enterprise/plugin-management`, `.../docs/build-skills`) contain **no documented version-comparison algorithm**, no semver-validation statement, no mention of whether a plugin listing can hold multiple versions simultaneously, and no mention of an install-time version-pin flag. All four are **undocumented** in what primary sources yielded.
- The only versioning-adjacent detail found: Agent Plugins 1.0's own `plugin.json` treats `version` as an optional field with "semantic versioning recommended" (i.e. a convention, not an enforced/parsed rule) — this is spec-level, not Codex-specific, and doesn't resolve the comparison-algorithm question either.

## Zip / upload surfaces

| Surface | Accepts a zip? | Root layout / manifest required | Size limit | Per-user or admin-only | Git marketplace also accepted here? |
|---|---|---|---|---|---|
| **claude.ai (Skills, in-product upload)** | Yes | A folder containing `SKILL.md`; folder name must match the skill name. No `.claude-plugin/plugin.json` requirement documented for this consumer-facing upload path (it is the plain-skill path, not the plugin path) | 30 MB (documented as "uncompressed"); also a documented 200-file-count ceiling (surfaced as a user-reported install error, not a stated policy number, so treat the count as anecdotal) | Per-user by default (Free/Pro/Max/Team/Enterprise); an org owner can turn off skill creation for members on Team/Enterprise | Not applicable to this path — this is upload-only; git-based marketplaces are a separate Claude Code mechanism, not part of the claude.ai Skills upload UI |
| **Claude Desktop** | Same skills-upload mechanism as claude.ai per the support article (skills enabled in claude.ai settings "also load in Claude Code in your terminal"); Desktop-specific zip-size/layout differences not separately documented | Same as above (undocumented Desktop-specific deltas) | Undocumented separately from claude.ai | Per-user, same org-owner override | Desktop's plugin browser (`+ > Plugins`) does separately support adding a marketplace by git/URL source, per the Claude Code install-plugins doc — that is the plugin path, distinct from the Skills zip-upload path |
| **Cowork** | Uses the same claude.ai-account plugin/skill sync rather than its own upload flow — synced plugins "download into the session's own environment when the session starts"; no separate Cowork-only zip upload documented | N/A (inherits from claude.ai account) | N/A | Per-user/per-org same as claude.ai | N/A — Cowork has no plugin browser of its own per the Claude Code docs |
| **Team/Enterprise org-managed plugin upload** | The org admin console page **Organization settings > Plugins & skills** turns plugins on for members' accounts; the managed-settings/allowlist docs explicitly note **"it doesn't check uploaded plugins"** with the allow/blocklist mechanism — i.e., admin-uploaded or member-uploaded plugins bypass the git-source allow/blocklist gate that governs marketplaces | Undocumented layout requirement beyond the general Skills zip rules above | Undocumented | Admin-controlled turn-on; the upload itself can be a member's own claude.ai upload turned on org-wide, per the org doc's "a marketplace made of a member's own claude.ai uploads" phrasing | Yes, separately — org settings also register git marketplaces via `extraKnownMarketplaces`/managed settings, which is the allow/blocklist-gated path; the two (uploads vs. git marketplaces) are explicitly documented as different gates |
| **Claude Code `archive` marketplace source** | Yes, this is a zip specifically for the *plugin* path (not the consumer Skills path): a `source: "archive"` entry with a `url` (HTTPS only, no loopback/link-local/cloud-metadata hosts) and optional `sha256` (64 hex chars) pin | "The plugin root may be at the top of the zip or one directory down." Needs a valid `plugin.json`/standard layout inside, like any other plugin source | No explicit size limit documented in the marketplace reference (unlike the 30 MB Skills-upload limit) | This is a marketplace-maintainer mechanism (whoever writes the `marketplace.json` entry), not an end-user upload UI | The same `marketplace.json` can mix `archive` entries with `github`/`git`/`npm`/`command` entries for other plugins — it's a per-plugin source choice, not an either/or with git marketplaces |

**Copilot (one line):** No documented zip/archive plugin-source type was found in the primary Copilot pages fetched — VS Code's Agent Plugins panel documents "Install from Source" (local directory) and standard VSIX packaging/installation for the extension host itself, but a dedicated "upload a zip to install an Agent Plugin" flow analogous to claude.ai's Skills upload was not found and is treated as **not confirmed / undocumented** for this research pass.

**Codex (one line):** No documented plugin-specific zip-upload path was found — ChatGPT's general file-upload limits (512 MB/file, no zip-unpacking: "ChatGPT does not unpack archives") are a different, unrelated surface, and the documented Codex plugin distribution paths are local-directory dev marketplaces (`~/.agents/plugins/marketplace.json`, `.agents/plugins/marketplace.json`) plus a submission portal into the shared ChatGPT/Codex plugin directory — no zip container format for that portal was found in the primary docs fetched.

## Key findings

1. Agent Plugins 1.0 is a real, named, cross-vendor spec (not just a marketing label) — schema at `https://agent-plugins.org/schemas/1.0.0/plugin.schema.json`, source at [github.com/agentplugins/agent-plugins-spec](https://github.com/agentplugins/agent-plugins-spec) (v1.0.0 published, v1.1.0 working draft at fetch time), GA'd 2026-08-06 per the [GitHub changelog post](https://github.blog/changelog/2026-08-12-agent-plugins-1-0-in-vs-code-copilot-cli-and-the-copilot-app/). Anthropic/Claude Code is not among the initial adopters or maintainers named in any primary source fetched.
2. The standard's scope is deliberately minimal: `plugin.json` + `skills/` + `mcp.json` only. Every other component type (agents, commands, hooks, LSP, rules) is reduced to a vendor-namespaced escape hatch (`extensions.<reverse-dns>` + a matching top-level directory like `com.github.copilot/` or `com.openai/`), explicitly so that "a client that doesn't recognize a given namespace simply ignores it" — this is opt-in interop for two components, not full plugin portability.
3. Copilot's plugin format is the only one of the three that documents an always-on **`rules/`** directory as a plugin component at all (`com.github.copilot/rules/`) — but it's Copilot-namespaced, so it doesn't travel to VS Code-as-a-different-vendor's namespace, Codex, or Claude Code. Claude Code plugins have no rules component whatsoever (CLAUDE.md at plugin root is explicitly rejected); Codex plugins have no documented rules component either (AGENTS.md sits outside the plugin system entirely).
4. VS Code is the one concrete, documented instance of a non-Claude client reading Claude's own manifest format (`.claude-plugin/plugin.json`) by name, including `${CLAUDE_PLUGIN_ROOT}` token substitution for MCP fields — this is the strongest interop evidence found in either direction. It is explicitly *not* full-fidelity in the docs retrieved: no statement that hooks, hard-coded `agents/`/`commands/` paths, `output-styles/`, or a Claude `marketplace.json` are handled the same way.
5. Claude Code's own versioning is intentionally non-semver: manifest `version` is documented as "not checked against semver," compared by plain string identity against the last-installed record, with git SHA / archive SHA-256 as the actual reproducibility mechanism when `version` is left unset.

## negative: searched, not found

- A published JSON Schema URL specifically for Claude Code's `plugin.json` (as opposed to the unrelated `claude-code-settings.json` schema on schemastore.org, and as opposed to the `$schema` field plugin authors may set but which Claude Code ignores at load time).
- A full, primary-sourced field table for GitHub Copilot's `marketplace.json` beyond `name`/`owner`/`metadata`/`plugins[].{name,description,version,source}` — the CLI reference gestures at a fuller field set without enumerating it in the pages retrieved.
- Any primary Copilot or Codex documentation of the version-comparison algorithm (string vs. semver precedence) or of whether a marketplace listing can hold multiple simultaneous versions of one plugin.
- Any documented zip/archive plugin-source type for Copilot, or a dedicated plugin-zip-upload flow for Codex/ChatGPT (distinct from generic file uploads, which explicitly do not unpack archives).
- Any statement, in either direction, of Claude Code reading an Agent Plugins 1.0 `plugin.json`/`mcp.json`, or of Copilot CLI / Copilot app (as opposed to VS Code specifically) recognizing `.claude-plugin/`.
- A distinct AAIF (per the prompt's naming) — no organization by that exact name was found; the operative cross-vendor body located is the Agent Plugins steering committee/charter at the agent-plugins-spec repo. agentskills.io appeared in search results as a third-party explainer site, not a standards body, and is treated here as commentary, not a primary source.

## leads:

- Confirm hands-on whether a Claude Code plugin with hooks/`agents/`/`output-styles/` installed via VS Code's "Install from Source" actually activates those components, or silently drops everything but skills+MCP — the docs stop short of saying, and this is the crux of the "would it install fully" question.
- Check whether Claude Code's `claude plugin validate` treats an Agent Plugins 1.0-schema `plugin.json` (with `$schema` set to the agent-plugins.org URL) as a warning-only "unrecognized top-level field" (degrades gracefully) or as a hard validation failure (name-charset rules etc. are stricter/looser between the two specs in ways that could collide).
- The Agent Plugins 1.0 GOVERNANCE.md / charter (steering-committee composition, "no vendor majority" rule) was referenced by a summarization pass but not independently re-verified line-by-line against the raw file — worth a direct read before citing the governance claim in a design doc.
- GitHub Copilot enterprise-managed plugins reached public preview 2026-05-06 per the changelog; re-check nearer the Expires date whether it has since gone GA and whether an allow/blocklist mechanism as granular as Claude Code's (`hostPattern`/`pathPattern`/owner-wildcards) has shipped.

## Sources

| Source | Type | Date | Relevance |
|--------|------|------|-----------|
| [code.claude.com/docs/en/plugins-reference](https://code.claude.com/docs/en/plugins-reference) | Docs (primary, Anthropic) | fetched 2026-09-27 | Full `plugin.json` manifest field reference, standard layout, CLAUDE.md rejection |
| [code.claude.com/docs/en/plugin-marketplaces](https://code.claude.com/docs/en/plugin-marketplaces) | Docs (primary, Anthropic) | fetched 2026-09-27 | `marketplace.json` walkthrough, plugin entry fields, source-type table |
| [code.claude.com/docs/en/plugins/marketplace-reference](https://code.claude.com/docs/en/plugins/marketplace-reference) | Docs (primary, Anthropic) | fetched 2026-09-27 | Full source-type field reference incl. `archive`/`command`, validation messages |
| [code.claude.com/docs/en/plugins/loading](https://code.claude.com/docs/en/plugins/loading) | Docs (primary, Anthropic) | fetched 2026-09-27 | Versions-and-updates algorithm, name-conflict precedence, synced plugins |
| [code.claude.com/docs/en/plugins/org](https://code.claude.com/docs/en/plugins/org) | Docs (primary, Anthropic) | fetched 2026-09-27 | Managed settings, allow/blocklist, seed dirs, uploaded-plugins carve-out |
| [code.claude.com/docs/en/plugins/install](https://code.claude.com/docs/en/plugins/install) | Docs (primary, Anthropic) | fetched 2026-09-27 | Install scopes, per-surface install flows, dependency handling |
| [code.claude.com/docs/en/plugins/cli-reference](https://code.claude.com/docs/en/plugins/cli-reference) | Docs (primary, Anthropic) | fetched 2026-09-27 | `claude plugin install/update/list` flags — no version-pin flag found |
| [support.claude.com/en/articles/12512180-use-skills-in-claude](https://support.claude.com/en/articles/12512180-use-skills-in-claude) | Help Center (primary, Anthropic) | fetched 2026-09-27 | claude.ai Skills zip-upload rules, org owner override |
| [docs.github.com/en/copilot/concepts/agents/about-plugins](https://docs.github.com/en/copilot/concepts/agents/about-plugins) | Docs (primary, GitHub) | fetched 2026-09-27 | Plugin concept overview, portability/namespacing model |
| [docs.github.com/en/copilot/reference/copilot-cli-reference/cli-plugin-reference](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-plugin-reference) | Docs (primary, GitHub) | fetched 2026-09-27 | `plugin.json` field reference for both Agent Plugins 1.0 and legacy Copilot formats |
| [docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/plugins-creating](https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/plugins-creating) | Docs (primary, GitHub) | fetched 2026-09-27 | Plugin creation guide, format choice |
| [docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/plugins-marketplace](https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/plugins-marketplace) | Docs (primary, GitHub) | fetched 2026-09-27 | `marketplace.json` structure (partial field set) |
| [docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/plugins-finding-installing](https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/plugins-finding-installing) | Docs (primary, GitHub) | fetched 2026-09-27 | Install/update commands; version semantics not documented here |
| [github.blog/changelog/2026-08-12-agent-plugins-1-0-in-vs-code-copilot-cli-and-the-copilot-app](https://github.blog/changelog/2026-08-12-agent-plugins-1-0-in-vs-code-copilot-cli-and-the-copilot-app/) | Changelog (primary, GitHub) | fetched 2026-09-27 | GA date, adopter list, marketplace story |
| [code.visualstudio.com/docs/agent-customization/agent-plugins](https://code.visualstudio.com/docs/agent-customization/agent-plugins) | Docs (primary, Microsoft/VS Code) | fetched 2026-09-27 | Format auto-detection table incl. Claude format, `${CLAUDE_PLUGIN_ROOT}` note, install scopes |
| [github.com/agentplugins/agent-plugins-spec](https://github.com/agentplugins/agent-plugins-spec) | Spec repo (primary) | fetched 2026-09-27 | Canonical Agent Plugins 1.0 spec, governance charter, version status |
| [learn.chatgpt.com/docs](https://learn.chatgpt.com/docs) | Docs (primary, OpenAI) | fetched 2026-09-27 | Doc index, plugin/skills/MCP page map |
| [learn.chatgpt.com/docs/build-skills](https://learn.chatgpt.com/docs/build-skills) | Docs (primary, OpenAI) | fetched 2026-09-27 | SKILL.md frontmatter, `agents/openai.yaml`, skill scopes |
| [learn.chatgpt.com/docs/enterprise/plugin-management](https://learn.chatgpt.com/docs/enterprise/plugin-management) | Docs (primary, OpenAI) | fetched 2026-09-27 | Workspace/role install policy; version semantics undocumented here |
| `developers.openai.com/plugins/build/plugins` (redirected/served content) | Docs (primary, OpenAI) | fetched 2026-09-27 | Universal plugin directory manifest, `extensions.com.openai`, local marketplace paths |
| WebSearch: GitHub issue/PR discussion on Copilot plugin versioning (`github/copilot-cli` issue #3129 and related PRs) | Secondary/community, flagged as such | fetched 2026-09-27 | Corroborating (not primary-confirmed) claim that Copilot `version` is informational and ref/SHA is the real pin |
| WebSearch: agentskills.io, scienceshot.com, codex.danielvaughan.com, explainx.ai, kenmuse.com blog posts | Secondary/blog, flagged as such | fetched 2026-09-27 | Directional corroboration only (adopter list, "Claude sat it out" framing); not cited for any fact not also found in a primary source above |
