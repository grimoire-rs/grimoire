# Research: Qoder plugin and marketplace format (for `grim export marketplace`)

## Metadata

**Date:** 2026-09-29
**Domain:** packaging
**Triggered by:** owner decision that `grim export marketplace` serves Qoder natively (own `.qoder-plugin/marketplace.json`, trees under `./qoder/<plugin>/`, Qoder family mapping)
**Expires:** 2027-03-29 (Qoder CLI ships weekly; re-verify on any `qodercli` minor bump)

Evidence tiers, strongest first: **[P]** probe run today against `@qoder-ai/qodercli@1.1.64` (npm `latest`, modified 2026-09-25, [npm registry](https://registry.npmjs.org/@qoder-ai/qodercli), accessed 2026-09-29) run from the unpacked bundle with `node`, throwaway `HOME` under the scratchpad (`pq/`); **[B]** static read of that bundle, `package/bundle/qodercli.js` (minified; symbol names below are that build's and will change); **[D]** docs.qoder.com; **[R]** real third-party repos.

## Direct Answer

1. Qoder reads `.qoder-plugin/plugin.json` first, `.claude-plugin/plugin.json` second [B `$Ne`: `Ktn=[".qoder-plugin",".claude-plugin"]`]. Both installed and validated with a Claude-generated manifest [P r1 vs r2]. The manifest is Claude-compatible byte for byte: same three keys grim emits (`name`, `version`, `description`) are all valid; only `name` is required [D, B]. Docs never mention `.claude-plugin`; the fallback is code-only.
2. Marketplace: `.qoder-plugin/marketplace.json` > `.claude-plugin/marketplace.json` > root `marketplace.json` [B `hVA`/`Hhi`]. **Confirmed independent catalogs**: a repo with both files listing different plugins exposes only the `.qoder-plugin` one [P both1: `pa` installed, `pb` "not found in any marketplace"]. Required: `name`, `owner`, `plugins` [D, B `Dii`]. Same shape as Claude's.
3. Commands: `qoder plugins marketplace add <source>`, `... install <plugin>[@<marketplace>]`, `... update [name]`. Interactive `/plugins`. Git URL forms are narrow (see Q3). The IDE does **not** read the marketplace file (see Q2).
4. Qoder cannot carry **rules** in a CLI plugin (no `rules/` component) and needs a Qoder-aware MCP branch (see Q4). Skills, agents, `.mcp.json` are Claude-identical.
5. Recommendation: **(b)** Qoder is the Claude family with the manifest directory `.qoder-plugin/` (one per-client parameter, not a new family), plus a Qoder carve-out in `mcp_value`. Details and smoke test in Recommendation.

## Q1. Plugin format

- Manifest path: `.qoder-plugin/plugin.json` ("recommended for every plugin", [plugins](https://docs.qoder.com/cli/plugins.md), 2026-09-29); fallback `.claude-plugin/plugin.json` [B `$Ne`, `mad`]. First hit wins, so a tree with both is read from `.qoder-plugin` only. [P r1 (Claude manifest) and r2 (Qoder manifest) both `plugins install` + `plugins validate` OK; cache dirs `1.0.0-abc123def456/.claude-plugin/` vs `.qoder-plugin/`.]
- Fields [D [plugins-reference](https://docs.qoder.com/cli/plugins-reference.md), 2026-09-29; B `$oe`]: required `name` (no spaces, kebab-case recommended); optional `version` (docs: semver; code: any string), `displayName`, `description`, `author` (object), `homepage`, `repository`, `license`, `keywords`, `dependencies`; component overrides `commands`, `agents`, `skills`, `outputStyles`, `workflowsPath(s)`, `hooks`, `mcpServers`, `settings` (only `agent` key honoured). `lspServers` and `channels` parse but warn as unsupported [B `$6A`]. Unknown keys tolerated (prior probe with `$schema`, `x-grim`, `unknownKey`: [research_marketplace_manifest_schemas.md:59](research_marketplace_manifest_schemas.md)).
- `version` semantics [B `y0`, `mdl`]: manifest `version ?? "local"`; `plugins update` compares **marketplace entry version** to installed version with `===`: equal skips, anything else (including a missing side) reinstalls. Opaque string, no semver ordering. `+build` accepted; on-disk dir rewrites `+` to `-` (`1.0.0-abc123def456`), `plugins list` shows `v1.0.0+abc123def456` [P]. grim's `X.Y.Z+<12 hex>` works as is.
- Component discovery [B `Ztn`; D]: `commands/`, `skills/`, `agents/`, `hooks/hooks.json` (needs `{ "hooks": ... }` wrapper), `output-styles/`, `workflows/`, `bin/`, `.mcp.json` **and** `mcp.json`. `plugins validate` errors with `plugin.installability.no_content` when none exists [P rl2]. No `rules/`.
- Placeholders: `${QODER_PLUGIN_ROOT}`, `${QODER_PLUGIN_DATA}` **and** `${CLAUDE_PLUGIN_ROOT}`, `${CLAUDE_PLUGIN_DATA}` are all expanded, in hook env and in MCP `command`, `args`, `env`, `url`, `httpUrl`, `headers` [B `Flt`, `LGI`/`uV`; docs name only the `QODER_` pair]. Claude-spelled placeholders work unchanged in Qoder.
- Byte-for-byte: **yes** for `.claude-plugin/plugin.json`, skills, agents, `.mcp.json` [P r1]. Agent frontmatter `tools: Read, Grep`, `model: inherit` validated [P rl].
- Agent Plugins root `plugin.json` is **not** read (`$Ne` only looks inside the two dot-dirs); an Agent Plugins tree would need a manifest dir added [B].

## Q2. Marketplace format

- Shape [D plugins-reference; B `Dii`, `Gtn`; R]: top level `name` (req), `owner{name,email?,url?}` (req), `plugins[]` (req), `metadata{version,description,pluginRoot}`, `forceRemoveDeletedPlugins`, `allowCrossMarketplaceDependenciesOn`. Entry: `name` (req, "must match plugin.json name" in docs, **not enforced**: entry `entryname` with manifest `realname` installed as `entryname` [P mm]), `source`, `version`, `description`, `displayName`, `category`, `tags`, `strict` (default `true`: manifest required in the plugin folder), plus every plugin-manifest field (`Gtn = $oe.partial().extend(...)`). `source` string must start `./` (`Path must start with ./`, no `..` rule in the string schema but containment is checked at copy) or an object `npm | pip | url | github | git-subdir` [B `Woc`]. Real files agree: `"source": "./apify"` [R [apify/apify-qoder-plugin](https://github.com/apify/apify-qoder-plugin/blob/61902ae/.qoder-plugin/marketplace.json)], `"./plugins/qoder-security"`, `strict:true`, `metadata.version` [R [liu-zey/qoder-plugins-official](https://github.com/liu-zey/qoder-plugins-official/blob/db7c91f/.qoder-plugin/marketplace.json), a copy; `QoderAI/qoder-plugins-official` is an empty repo today].
- Lookup [B `hVA`, `Hhi`; P both1, bad1]: `.qoder-plugin/marketplace.json`, `.claude-plugin/marketplace.json`, root `marketplace.json`; `hVA` returns the first that **exists**, so an existing-but-invalid `.qoder-plugin` file is a hard error, not a fallthrough [P bad1: "Invalid JSON in marketplace manifest (…/.qoder-plugin/marketplace.json)"]. Docs state only "Location: `marketplace.json`"; the second-place rank is code-only.
- Names [B `sBe`,`Roc`,`XNe`; P nm-*]: marketplace name `/^[a-z0-9][-a-z0-9._]*$/i`, 1-100 chars, printable ASCII, no space; rejected as "reserved or impersonates an official marketplace" when it matches `official` next to `qoder`, `qoder`+`official`, or starts `qoder`+(`marketplace`|`plugins`|`official`), **except** the exact strings `qoder-marketplace`, `qoder-plugins`, `qoder-plugins-official` (allowlist `Poc`, accepted by a local add [P]); also rejected `inline`, `builtin`, `local`, `flag`; namespaces `qoder-enterprise-*` and `qoderwork-enterprise-*` reserved. Probed: `qoder-plugins-x`, `my-qoder-official`, `Qoder-Official`, `qoder-enterprise-x` rejected; `qoder-tools` accepted. The prior note "reserves `qoder-` prefix" is too broad: `qoder-tools` passes. Plugin entry names: only "no spaces" enforced [B `Gtn`].
- Updates: `marketplace update [name]` re-fetches the catalog; `plugins update <plugin>` reinstalls when entry `version` differs from installed (above). An entry without `version` reinstalls every time. `forceRemoveDeletedPlugins` (optional) uninstalls plugins that left the catalog.
- **IDE/Desktop/QoderWork ignore the file**: "`.qoder-plugin/marketplace.json` is **Qoder-CLI-only**. The GUI surfaces (IDE, Desktop app, QoderWork) ignore it and take listing metadata from the publish form" [R [apify RELEASE.md](https://github.com/apify/apify-qoder-plugin/blob/61902ae/RELEASE.md), 2026-09-29]. Publishing is a manual web form (qoder.com > My Publications) that takes a ZIP; the enterprise private marketplace states "ZIP root directory must contain a `.qoder-plugin/` directory" [D [enterprise marketplace](https://docs.qoder.com/account/enterprise/marketplace.md)]. Whether the IDE's "Upload Plugin" accepts a `.claude-plugin/`-only ZIP is **unverified** (no IDE access).

## Q3. Add and install

- CLI [D plugins.md; P]: `qoder plugins marketplace add|list|remove|update`, alias `mp`; `qoder plugins install <name>[@<marketplace>] [--scope user|project|local]`, `uninstall|enable|disable|update|validate|list`. Binary is `qodercli` on npm; `qoder` per docs. Plugin id `name@marketplace`.
- Source forms [B `Hrt`; P]: `user@host:path[.git][#ref]` (scp-style SSH), `http(s)://…` ending `.git` or containing `/_git/` (git clone), `https://github.com/owner/repo` (auto-appends `.git`), **any other `http(s)` URL is treated as a direct `marketplace.json` URL, not a git repo** (a GitLab `https://gitlab.com/o/r` without `.git` becomes a failed JSON fetch), local dir or `.json` file, `owner/repo[#ref|@ref]` (GitHub only, tries `git@github.com:` SSH first, then HTTPS). `file://` and `ssh://` URLs: `file://` rejected "Invalid marketplace source format" [P]; `ssh://` fails the scp regex [B].
- Private repos [B `LsA`]: clone is `git clone --depth 1` with `GIT_TERMINAL_PROMPT=0`, `GIT_ASKPASS=""`, `GCM_INTERACTIVE=never`, `-c credential.interactive=false`, a fixed `core.sshCommand`. So a private repo works only with a non-interactive credential (SSH agent/key, stored helper); no prompt, no token flag. Docs are silent ([plugins](https://docs.qoder.com/cli/plugins.md)). Timeout default 120 s.
- `qoder plugins marketplace add <dir>` with the generated repo cloned locally is fully scriptable offline [P]: `add`, `install foo@probe-one`, `list`, `validate` all ran with no login.

## Q4. What a Qoder plugin cannot carry that Claude can

| Kind | Claude family tree | Qoder |
|---|---|---|
| skills | `skills/<n>/` | identical [P] |
| agents | `agents/<n>.md` | identical, Claude frontmatter ([vendor_qoder.rs:14-15](../../src/install/vendor_qoder.rs)) [P rl] |
| rules | none (already `NoFormatSurface`, family.rs:110) | none either way: no plugin `rules/` loader in 1.1.64 (the rules handler enumerates only user, project and walk-up dirs) [B `PFs`; P rl2 rules-only plugin fails validation]. IDE docs list "rules" among plugin contents ([Qoder plugins](https://docs.qoder.com/qoder/plugins.md)), unverified for the CLI |
| mcp | `.mcp.json` `mcpServers` | `.mcp.json` and `mcp.json` both loaded; `type` in `stdio|sse|http|ws|streamable-http` (`streamable-http` normalised to `http`), absent `type` with only `url` means http [B `xAo`, `ZNn`]; a Claude `{type:"http",url}` works. Apify ships both `url` and `httpUrl` "for cross-host compatibility" (Qoder's own key is `httpUrl`) [R] |
| commands, hooks, output-styles, workflows, bin | not emitted by grim | supported, not needed |

**MCP env refs (correction to the watchlist).** [vendor-capability-watchlist.md](../../.claude/rules/vendor-capability-watchlist.md) (Qoder rows, "MCP env-ref expansion") says no substitution exists. The bundle expands `${VAR}` / `${VAR:-default}` from `process.env` in `url`, `httpUrl`, `headers` values and stdio `env` values at connect time [B `ES`, `jNn`, transport builder], and plugin placeholders everywhere above; not in `command`/`args` (only plugin placeholders there). Docs remain silent. Candidate for `/upstream-refresh`.

## Q5. grim today

- `family_of` maps Claude, Droid, Junie, OpenClaw to `Family::Claude`; Qoder falls in `_ => None` ([family.rs:82-89](../../src/export/family.rs)). So `grim export plugin --client qoder` exits 78 `NoPluginFormat` ([export.rs:271-281](../../src/command/export.rs)); a `[options].clients` list containing qoder is dropped with a warn (export.rs:286-290); all-none falls back to `[agents]`.
- `admits(Family::Claude, Qoder, kind)` would give skill/agent/mcp Ok, rule `NoFormatSurface` ([family.rs:107-119](../../src/export/family.rs)): `QoderVendor.kind_support` is `Native` for all four kinds ([vendor_qoder.rs:199](../../src/install/vendor_qoder.rs)).
- **The catch is MCP.** `mcp_value(Family::Claude, client, …)` ([stage.rs:1075-1096](../../src/export/stage.rs)) calls `client.vendor().mcp_entry(Global, …)`. `QoderVendor::mcp_entry` returns `None` for any descriptor with `${...}` ([vendor_qoder.rs:99-108](../../src/install/vendor_qoder.rs)), because for `settings.json` installs expansion is undocumented. In a plugin `.mcp.json` that would omit every server using `${PLUGIN_ROOT}` or `${CLAUDE_PLUGIN_ROOT}` as `not-representable`, though Qoder expands both. It also skips the Claude-only rename branch, which is gated `client == ClientTarget::Claude` (stage.rs:1085-1094) with the comment "other Claude-family clients keep the descriptor's spelling (their plugin expansion is unverified)". For Qoder it is now verified [B]: treat Qoder like Claude there (rename `PLUGIN_*` to `CLAUDE_PLUGIN_*`, keep other refs).
- The prior research claim "Qoder reads `.qoder-plugin/` then `.claude-plugin/`" is **verified** for the plugin manifest and the marketplace file [B, P]; [research_marketplace_manifest_schemas.md:57-62](research_marketplace_manifest_schemas.md) also says a separate `.qoder-plugin/marketplace.json` is "not needed"; that stays true for the CLI but the owner decision (own file, own tree) is compatible with it, and Qoder-only ZIP/IDE flows favour it (Q2).
- Watchlist Qoder rows ([lines 217-230](../../.claude/rules/vendor-capability-watchlist.md)) list only support-dir exclusion and env-ref expansion; neither concerns plugins. Add a row: plugin manifest fallback and marketplace lookup order, dated 2026-09-29, CLI 1.1.64.

## Recommendation

**(b): Qoder is `Family::Claude` with the manifest directory `.qoder-plugin/`.** Tree = the Claude tree bytes (`skills/`, `agents/`, `.mcp.json`, `README.md`, `assets/logo.*`) but `plugin.json` at `.qoder-plugin/plugin.json`, content identical to `claude_plugin_json` output (three keys). Reasons, max three:

1. **Documented path, zero ambiguity.** `.qoder-plugin/plugin.json` is the only manifest path the docs name (plugins, SDK, enterprise ZIP rule); `.claude-plugin` is an undocumented fallback that a Qoder release could drop. Same bytes, so no second emitter: one `Family::Claude` plus a per-client manifest dir constant (Claude, Droid, Junie, OpenClaw keep `.claude-plugin`).
2. **(a) is weaker, (c) is unwarranted.** (a) works in the CLI [P r1] but relies on the fallback and likely fails the IDE/enterprise ZIP rule (`.qoder-plugin/` required). (c) has no divergence to justify a family: the only Qoder deltas are the manifest dir and the MCP branch.
3. **Free coexistence.** `.qoder-plugin/` shadows `.claude-plugin/` per plugin and per marketplace [P both1], so the Claude tree at `./claude/<plugin>` and Qoder tree at `./qoder/<plugin>` cannot cross-contaminate.

Required companion changes (all small): `family_of(Qoder) = Some(Claude)`; a `manifest_dir(client)` used by the writer and by `grim export marketplace` for the Qoder file location; `mcp_value` Qoder branch (translate `PLUGIN_*` placeholders, do not use the `settings.json` env-ref skip); marketplace file `.qoder-plugin/marketplace.json` with `name` (avoid the `qoder` + `marketplace|plugins|official` and `official`+`qoder` patterns, no `qoder-enterprise-*`), `owner.name`, entries `{name, source: "./qoder/<plugin>", version, description}` and the same version string as the tree manifest (Qoder's update check is `===`); omit `strict` (default true is satisfied), omit `metadata.pluginRoot`. Do not emit a `rules/` dir.

**Consequence for `grim export plugin --client qoder`:** today exit 78 `NoPluginFormat`; after: a directory (or `--zip`) rooted at `.qoder-plugin/plugin.json`, `skills/`, `agents/`, `.mcp.json`, `README.md`. The `--zip` output also satisfies the IDE/enterprise upload rule (root contains `.qoder-plugin/`), unverified end to end. Qoder joins the `plugin_client_names()` hint and `select_clients`' configured-client path (no more warn-and-drop).

**Upstream uncertainties and how a smoke test settles them**

| Uncertainty | Settled by |
|---|---|
| `.claude-plugin` fallback is code-only (docs silent); `.qoder-plugin` lookup rank second for marketplace is code-only | keep a CI smoke: `qodercli plugins marketplace add <generated repo>`, `install <p>@<m>`, `list` asserts the exact version; scripted offline as in [P] |
| Update detection is `===` on entry version | smoke: bump tree, re-export, `qodercli plugins update <p>`, assert `updated` and new `list` version |
| IDE ignores the marketplace file; whether IDE ZIP import accepts the exported ZIP | manual once in the IDE (Extensions > Plugins > Add Plugins > Upload) with `grim export plugin --client qoder --zip`; cannot be automated |
| MCP env/placeholder expansion in `.mcp.json` (bundle-only) | smoke: export a plugin whose `.mcp.json` uses `${CLAUDE_PLUGIN_ROOT}`, `qodercli mcp list` inside it, assert the expanded path |
| Reserved-name regex may change | export validates names against the regex above and fails 65 before write |
| Private clone has no token path | document: private Qoder marketplaces need SSH or a stored git credential; GitLab HTTPS needs `.git` suffix |

## Sources

| Source | Type | Date | Relevance |
|--------|------|------|-----------|
| https://docs.qoder.com/cli/plugins.md | Docs (summarising fetch) | 2026-09-29 | scopes, marketplace add forms, `QODER_PLUGIN_*`, `name@marketplace` |
| https://docs.qoder.com/cli/plugins-reference.md | Docs (summarising fetch) | 2026-09-29 | plugin.json and marketplace.json fields, CLI commands, dir conventions |
| https://docs.qoder.com/cli/sdk/plugins.md | Docs | 2026-09-29 | `.qoder-plugin/plugin.json`, no `.claude-plugin` mention |
| https://docs.qoder.com/account/enterprise/marketplace.md | Docs | 2026-09-29 | ZIP root must contain `.qoder-plugin/` |
| https://docs.qoder.com/qoder/plugins.md | Docs | 2026-09-29 | IDE plugin components incl. rules; upload flow |
| https://docs.qoder.com/llms.txt | Index | 2026-09-29 | doc page inventory |
| https://registry.npmjs.org/@qoder-ai/qodercli (1.1.64) | Package, bundle read + probes | 2026-09-29 | manifest and marketplace lookup order, name rules, source parsing, update check, MCP transport and placeholders |
| https://github.com/apify/apify-qoder-plugin (`61902ae`) | Repo | 2026-09-29 | real marketplace + plugin, RELEASE.md: CLI-only marketplace file, GUI ignores it |
| https://github.com/liu-zey/qoder-plugins-official (`db7c91f`) | Repo (copy of official layout) | 2026-09-29 | marketplace shape with `strict`, `metadata.version` |
| https://github.com/leezhian/resume-session/issues/1 | Issue | 2026-09-29 | third-party report quoting the two fallback chains |
| /home/mherwig/dev/grimoire-duo/src/export/family.rs:82-119 | Repo | 2026-09-29 | family map and admission gate |
| /home/mherwig/dev/grimoire-duo/src/export/stage.rs:1075-1096 | Repo | 2026-09-29 | `mcp_value` Claude branch |
| /home/mherwig/dev/grimoire-duo/src/install/vendor_qoder.rs:83-143 | Repo | 2026-09-29 | `mcp_entry` env-ref skip |
| /home/mherwig/dev/grimoire-duo/src/command/export.rs:252-290 | Repo | 2026-09-29 | `select_clients`, `NoPluginFormat` |
| /home/mherwig/dev/grimoire-duo/.agents/research/research_marketplace_manifest_schemas.md:55-62 | Prior research | 2026-09-29 | earlier Qoder probes, verified here |

## Search terms (durable)

`qodercli plugins marketplace add`, `.qoder-plugin/plugin.json`, `.qoder-plugin/marketplace.json`, `QODER_PLUGIN_ROOT`, `CLAUDE_PLUGIN_ROOT` qoder, `@qoder-ai/qodercli` npm, `qoder plugins validate`, `qoder-plugins-official`, `qoder-enterprise-` reserved namespace, `preserveUpstreamMetadata`, `forceRemoveDeletedPlugins`, `docs.qoder.com/cli/plugins-reference.md`, `docs.qoder.com/llms.txt`, `path:.qoder-plugin filename:marketplace.json` (GitHub code search), qoder "Upload Plugin" ZIP `.qoder-plugin`, `agentsMdExcludes`.
