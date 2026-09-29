# Research: Codex/ChatGPT marketplace consumers and plugin icon fields

## Metadata

- **Date**: 2026-09-29
- **Domain**: packaging
- **Triggered by**: e2e marketplace shows no plugin icons; user claim "Codex merged into ChatGPT, same standard"
- **Expires**: 2027-03-29
- **Method**: WebFetch/WebSearch of vendor docs (small-model summaries, quotes not byte-checked); no live client run. Extends `research_agent_plugins_spec_verify.md` (2026-09-27).

## Q1. Who reads the Codex marketplace format

**Naming (user claim: mostly right).** On 2026-07-09 OpenAI merged the standalone Codex desktop app into the ChatGPT desktop app (Chat, Work, Codex in one shell; old ChatGPT app became "ChatGPT Classic"). Codex is not renamed away: it stays a named surface inside ChatGPT, and the Codex CLI keeps its name ([Addigy note](https://support.addigy.com/hc/en-us/articles/53439709931155-ChatGPT-and-Codex-Desktop-App-Merger-What-This-Means-for-Your-Fleet) and [Coursiv](https://coursiv.io/blog/codex-merged-with-chatgpt-app), via search snippet; the openai.com post and the Addigy page returned 403, so the date is secondary-sourced). OpenAI's own docs say "the ChatGPT desktop app" reads marketplaces and `developers.openai.com/codex/plugins` now 308-redirects to `learn.chatgpt.com/docs/plugins`.

| Client | File(s) read | Add-marketplace | Source |
|---|---|---|---|
| Codex CLI | `.agents/plugins/marketplace.json` (repo or `~/.agents/plugins/`); plugin: root `plugin.json` (Agent Plugins), fallback `.codex-plugin/plugin.json` | `codex plugin marketplace add owner/repo [--ref] [--sparse]`; also URL, local path; `list`/`upgrade`/`remove` | [Codex build docs](https://developers.openai.com/codex/plugins/build) |
| ChatGPT desktop app (Codex tab) | same repo/personal `.agents/plugins/marketplace.json`; restart app to see it in the Plugins Directory | drop the file in a repo/`~`, restart | [OpenAI plugin packaging](https://developers.openai.com/plugins/build/plugins) |
| ChatGPT workspace (admin) | one of `.agents/plugins/marketplace.json`, `.claude-plugin/marketplace.json`, or standalone `.claude-plugin/plugin.json` | Admin > Plugins > Add > Import marketplace; repo URL, optional subdir, ref; GitHub auth | [ChatGPT plugin management](https://learn.chatgpt.com/docs/enterprise/plugin-management) |
| ChatGPT web/mobile | consumes the shared plugin directory only (no documented git add for end users) | none documented | [ChatGPT plugins](https://learn.chatgpt.com/docs/plugins) |
| Copilot CLI | `.github/plugin/marketplace.json`, else `.claude-plugin/marketplace.json`; does not read `.agents/plugins/` | `copilot plugin marketplace add owner/repo` | [GitHub docs](https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/plugins-marketplace) |
| VS Code Copilot | plugin: root `plugin.json` (Agent Plugins 1.0), `.claude-plugin/plugin.json`; marketplaces are git repos | setting `chat.plugins.marketplaces` (`owner/repo`, https `.git`, scp, `file:///`); no command | [VS Code docs](https://code.visualstudio.com/docs/agent-customization/agent-plugins) |
| Cursor | `.cursor-plugin/marketplace.json`; plugin root `plugin.json` or `.cursor-plugin/plugin.json` | Dashboard > Plugins & MCPs > Team Marketplaces > Add / Import from Repo | [Cursor docs](https://cursor.com/docs/plugins), `research_cursor_plugin_probe.md` |
| Qoder CLI | Claude-shape `.qoder-plugin/` | `qoder plugins marketplace add <git/owner-repo/dir/url>` | [Qoder docs](https://docs.qoder.com/cli/plugins) |

Other Agent Plugins adopters (Amazon, Cursor, Microsoft, OpenAI, Vercel, Google as TSC members) are in the 2026-09-27 note. Note the split: the **plugin** manifest (`plugin.json`) is the open standard; the **marketplace** file is per-vendor (`.agents/plugins/` is Codex's, not part of agent-plugins.org). Codex docs also say entries need `source` as an object (`{source, path}`), `policy.installation`, `policy.authentication` and `category` (summary, verify live).

## Q2. Icon fields per client

| Client | Icon field | Where | Path/URL, limits |
|---|---|---|---|
| Claude Code / desktop / web | **none exists.** `plugin.json` fields: name, displayName, version, description, author, homepage, repository, license, keywords, metadata (free-form, unread), ...; marketplace entry: none either. Requests [#28187](https://github.com/anthropics/claude-code/issues/28187) (2026-02-24) and [#49040](https://github.com/anthropics/claude-code/issues/49040) (2026-04-16) for `icon` both closed "not planned"/stale, no Anthropic reply | n/a | n/a. Sources: [manifest ref](https://code.claude.com/docs/en/plugins-reference), [marketplace ref](https://code.claude.com/docs/en/plugins/marketplace-reference) |
| Codex / ChatGPT | `extensions."com.openai".interface.logo` and `.composerIcon` (also `screenshots`, `brandColor`, `displayName`, `shortDescription`, `category`...) | in `plugin.json` (root, under `extensions`), not the marketplace entry | relative path starting `./`, inside plugin root, docs suggest `./assets/`; docs state no size/format limits. Shown in ChatGPT Plugins Directory and composer. [source](https://developers.openai.com/plugins/build/plugins) |
| Copilot (CLI, VS Code) | none. Agent Plugins schema has no icon/logo property (`additionalProperties: false`, only `extensions` namespaces); GitHub/VS Code docs mention none | n/a | n/a |
| Qoder | none documented (fields: name, version, description, author, homepage, repository, license, keywords) | n/a | n/a |
| Cursor (extra) | `logo` in `.cursor-plugin/plugin.json` schema | plugin manifest | format/size undocumented in fetched docs |

So only ChatGPT/Codex (and Cursor) can show a manifest-declared icon today. Claude, Copilot and Qoder show a default icon whatever we emit.

## Our generated repo (`.agents/worktrees/e2e-marketplace`)

- `marketplace.toml`: no `logo` anywhere (`[plugins.grim-essentials]`, `[plugins.hex]` set only `include` and `description`). `PluginDecl.logo` exists (`src/export/marketplace.rs:89`, `.png`/`.svg`, path relative to the manifest, 1 MiB cap `stage.rs:1529`).
- Rendered trees: no `assets/logo.*` in any of `claude/ codex/ copilot/ qoder/`; no `extensions` in `codex/*/plugin.json` or `copilot/*/plugin.json`; READMEs have no logo image.
- Fallback: `stage.rs:257` tries the published repository companion logo (`logo.svg`/`logo.png` from the `__grimoire` tag) for a single-ref plugin; both e2e plugins are single-include but produced nothing, so those registry repos publish no companion logo (or the fetch failed quietly at debug level, `stage.rs:1513`).
- grim's writer (`family.rs:244-251`): Codex and Copilot share the Agent Plugins manifest, so with a logo both get `extensions."com.openai".interface.logo = "./assets/logo.<ext>"` (Copilot ignores it). Claude and Qoder `plugin.json` never carry a logo field (correct, none exists); the logo only lands as `assets/logo.*` plus a README image.
- Marketplace entries (`marketplace_export.rs:737` `EntryDoc`): `name, source, version, description` only. No `category`/`policy`, and `source` is a string in `.agents/plugins/marketplace.json`; Codex docs require an object `source`, `policy`, `category` (unverified live; could make Codex reject the entries).

## Exact gap

- **ChatGPT/Codex icon: (i) only.** grim already writes the field the client reads (`extensions."com.openai".interface.logo`); nothing is declared and no companion logo exists. Fix: add `logo = "assets/logo.svg"` (or `.png`) to each `[plugins.*]` in `marketplace.toml`, or publish a companion logo. Then check that Codex accepts `.svg` (docs silent; prefer `.png`). Caveat: ChatGPT admin import may pick `.claude-plugin/marketplace.json` over `.agents/...` (docs list all three, order unstated); a Claude-format plugin has no icon field, so icon needs the `.agents` path.
- **Claude icon: (ii), not fixable by grim.** Claude Code has no icon field; declaring a logo changes nothing there. Do not invent an `icon` key (unknown top-level keys are stripped, validate warns).
- **Copilot, Qoder: same as Claude**, no field.
- **Separate risk**: Codex marketplace entries lack `policy`/`category` and use a string `source`; verify against a real `codex plugin marketplace add` before claiming ChatGPT/Codex compatibility.

## Open

- Codex logo formats/sizes unspecified; SVG support unverified.
- ChatGPT admin-import precedence between `.agents` and `.claude-plugin` files unverified.
- Merge date rests on secondary sources (openai.com 403).

## Q3. Marketplace-level logo/icon (checked 2026-09-29)

Verdict: **no harness marketplace file has a marketplace-level icon or logo field.** Only the plugin manifest carries one (ChatGPT/Codex `extensions."com.openai".interface.logo`, Cursor `logo`). A `[marketplace].logo` key in grim would have nothing to render into, so it is not implemented.

| Client | Marketplace file | Fields that exist at marketplace level | Logo/icon | Source |
|---|---|---|---|---|
| Claude Code / Claude desktop / claude.ai | `.claude-plugin/marketplace.json` | `name`, `owner{name,email,url}`, `plugins`, `$schema`, `description`, `version`, `metadata.{description,version,pluginRoot}`, `forceRemoveDeletedPlugins`, `allowCrossMarketplaceDependenciesOn`, `renames`. Unknown keys are ignored, `plugin validate` warns | none | [marketplace reference, top-level fields](https://code.claude.com/docs/en/plugins/marketplace-reference#top-level-fields) |
| Codex CLI / ChatGPT | `.agents/plugins/marketplace.json` | `name`, `interface.displayName` ("the marketplace title shown in the ChatGPT desktop app"), `plugins` | none (`interface` holds only `displayName`) | [Codex build guide](https://developers.openai.com/codex/plugins/build) |
| Copilot CLI / VS Code | `.github/plugin/marketplace.json` | `name`, `owner{name,email?}`, `plugins`, `metadata.{description,version,pluginRoot}` | none | [Copilot CLI plugin reference](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-plugin-reference) |
| Qoder | `.qoder-plugin/marketplace.json` (Claude shape) | `name`, `owner`, `plugins`, `metadata.{version,description,pluginRoot}`, `forceRemoveDeletedPlugins`, `allowCrossMarketplaceDependenciesOn` | none | [Qoder plugins reference](https://docs.qoder.com/cli/plugins-reference.md), `research_qoder_plugin_marketplace.md` |
| Cursor | `.cursor-plugin/marketplace.json` | `name`, `owner{name,email}`, `metadata.description`, `plugins[]` of `name`, `source`, `description`, optional `minClientVersions` (read from the official `cursor/plugins` repo; the docs publish no marketplace schema) | none seen. `logo` exists only in each plugin's `.cursor-plugin/plugin.json` | [cursor/plugins marketplace.json](https://github.com/cursor/plugins/blob/main/.cursor-plugin/marketplace.json), [plugin reference](https://cursor.com/docs/reference/plugins) |
| ChatGPT workspace admin import | reads one of the three files above | n/a | docs silent on any icon | [plugin management](https://learn.chatgpt.com/docs/enterprise/plugin-management) |

Caveat: doc pages were read through WebFetch summaries, and Cursor's is inferred from one official sample, so a Cursor field could exist undocumented.

## Q4. GUI paths to add a marketplace (checked 2026-09-29)

- **Claude (claude.ai, Claude Desktop Chat tab, Cowork):** Customize > Plugins > Add > Add marketplace > Add from a repository, then `owner/repo` or a repository URL (GitHub, public GitLab and Bitbucket). All paid plans. [Use plugins in Claude](https://support.claude.com/en/articles/13837440-use-plugins-in-claude).
- **ChatGPT workspace admin:** Admin > Plugins > Add > Import marketplace, repository URL, optional subdirectory and ref, GitHub authorization; reads `.agents/plugins/marketplace.json`, `.claude-plugin/marketplace.json` or `.claude-plugin/plugin.json`. [Plugin management](https://learn.chatgpt.com/docs/enterprise/plugin-management), also [OpenAI Help: importing and syncing marketplaces](https://help.openai.com/en/articles/20001504-importing-and-syncing-plugin-marketplaces-from-github) (search snippet; 403 to fetch).
- **ChatGPT desktop app (Work mode / Codex):** no documented in-app "add marketplace" control. It reads `$REPO_ROOT/.agents/plugins/marketplace.json` and `~/.agents/plugins/marketplace.json`; restart the app, then each marketplace is a source in the Plugins Directory. [OpenAI plugin packaging](https://developers.openai.com/plugins/build/plugins). `codex plugin marketplace add` registers a source for the CLI.
- **VS Code Copilot:** no UI, setting `chat.plugins.marketplaces` only ([VS Code docs](https://code.visualstudio.com/docs/agent-customization/agent-plugins)).
