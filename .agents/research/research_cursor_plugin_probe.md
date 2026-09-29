# Research: Cursor plugin tree probe (`.cursor-plugin/plugin.json` vs root `plugin.json`)

## Metadata

**Date:** 2026-09-29
**Domain:** packaging
**Triggered by:** `adr_harness_marketplace_export.md` D2 gates Cursor on a "first spike" probe (Round-1 gap #1, Validation "Plan spike 1")
**Expires:** 2027-03-29 (Cursor plugin docs moved twice since 2.5; re-verify before 1.0 tag and at each Cursor minor)

Method and limits: docs and public repos only, fetched 2026-09-29 through WebFetch/WebSearch. WebFetch answers come from a small summarizer, so quotes below are as returned, not byte-checked against the page; the load-bearing ones (schema `additionalProperties`, discovery step 1) were each fetched twice with consistent results. No live Cursor was run. `which cursor-agent cursor` finds neither on this machine (nothing to version-report, nothing logged in).

## Direct Answer

1. **Does a Cursor marketplace entry need `<plugin>/.cursor-plugin/plugin.json`?** The documented parse says so: for `"source": "my-plugin"` the parser "looks for `my-plugin/.cursor-plugin/plugin.json`"; if found it is merged with the entry (manifest wins); component discovery then runs in the directory "using manifest paths if specified or folder-based discovery as fallback". Root `plugin.json` is **not** in that documented lookup. A root-only tree therefore degrades to "entry fields + folder discovery" and never gets its manifest read. Whether that degraded load still yields a working plugin is **not settled by docs**.
2. **Does Cursor accept Agent Plugins 1.0?** Yes, in general ("A plugin that conforms to the Agent Plugins specification loads in Cursor"; submission checklist: "valid root `plugin.json` or `.cursor-plugin/plugin.json`"). The docs never say the marketplace-import path takes that branch.
3. **The ADR's "same bytes" remedy is wrong.** Cursor's own `plugin.schema.json` has `additionalProperties: false` and lists no `$schema` and no `extensions`. grim's Agent Plugins `plugin.json` starts with `$schema` and carries `extensions` when there is a logo, so a byte copy at `.cursor-plugin/plugin.json` fails Cursor's own validator. The bytes of grim's *Claude-family* manifest (`{name, version, description}`) are already a valid Cursor manifest.
4. **Settled from docs alone?** Partly. The file requirement and the "same bytes fails lint" point are settled on paper. Three items need a live install (see Open questions).

## Findings

### 1. Cursor manifest

- Two manifest locations, by format: Agent Plugins = `plugin.json` at the plugin root; Cursor Plugins = `.cursor-plugin/plugin.json`. Source: https://cursor.com/docs/plugins and https://cursor.com/docs/reference/plugins (both 2026-09-29).
- Only `name` is required (kebab-case, `^[a-z0-9]([a-z0-9.-]*[a-z0-9])?$`). `version` is optional, "Semantic version". Schema: https://github.com/cursor/plugins/blob/main/schemas/plugin.schema.json (raw fetched 2026-09-29): `version` is a free string with **no `pattern`**, so `1.4.0+3f9a0c12b7de` passes the schema (and is valid semver build metadata).
- `plugin.schema.json` root properties: `name displayName description version minClientVersions author publisher homepage repository license logo keywords category tags commands agents skills rules hooks variables mcpServers`; `additionalProperties: false`. No `$schema`, no `extensions`.
- Version semantics: docs give no update rule. Prior research (`research_plugin_support_matrix.md:48`): update = git ref refresh, not version compare. Marketplace auto-refresh reindexes at most once per 10 minutes (docs).
- Agent Plugins in Cursor: "Cursor does not expand the standard's `${PLUGIN_ROOT}` and `${PLUGIN_DATA}` variables in `mcp.json`" (reference page). grim renames Claude's placeholders to exactly those for every Agent Plugins client, Cursor included (`src/export/family.rs:307-308`), and `stage.rs:1101-1112` warns only about *other* env refs. An MCP entry using `${PLUGIN_ROOT}` will not resolve in Cursor. Separate defect from the probe; flag it.

### 2. Cursor marketplace file

- Path `.cursor-plugin/marketplace.json` at repo root. Docs: required `name`, `owner{name,email?}`, `plugins[]`; optional `metadata{description,version,pluginRoot}`; file <= 10 MB.
- Entry fields in docs: `name` (required), `source` (string or object), `description`, `version`, `author`, ... Official repo entries are `{name, source: "teaching", description}` with **bare** sources (https://raw.githubusercontent.com/cursor/plugins/main/.cursor-plugin/marketplace.json).
- `marketplace.schema.json` (raw, 2026-09-29): plugin entry allows only `name`, `source` (string, `minLength: 1`, **no pattern**), `description`, `minClientVersions`; entry `additionalProperties: false`; root allows `$schema`. So grim's D3 entry (`version`) fails the **repo lint schema** but matches the **docs** list. Prior file `research_marketplace_manifest_schemas.md:27,38` already recorded this split.
- `./x` sources: schema places no constraint. Real files exist both ways: stripe/ai ships `./providers/cursor/plugin/` (prior research, `...schemas.md:33`); wshobson/agents ships local-path sources plus entry `version`, `author`, `license` (https://raw.githubusercontent.com/wshobson/agents/main/.cursor-plugin/marketplace.json); toolboxmd/marketplace points at `./cursor/agentsmd` (https://github.com/toolboxmd/marketplace). Counter-evidence: `source: "."`/`"./"` is dropped by Cursor's GitHub importer (https://github.com/blindrelay-app/agent-plugin/pull/5, https://github.com/ArefMozafari/pr-evidence/issues/9). Only the repo-root case failed; `./sub/dir` is the working shape in every example found. `./<client>/<plugin>` is therefore very likely fine.
- How a team adds a git marketplace: **dashboard**: Dashboard -> Settings -> Plugins -> Team Marketplaces -> Add/Import from Repo (GitHub, GitLab, Bitbucket, Azure DevOps URL), plan Teams/Enterprise; auto-refresh needs the Cursor GitHub App and GitHub source (docs; https://docs.withwillow.ai/docs/admin/install-marketplace/cursor). **CLI exists but is interactive**: `cursor-agent plugin marketplace add <repository-url>`, then `/plugin` -> Marketplace tab; staff, 2026-07-19: no non-interactive `plugin install` yet (https://forum.cursor.com/t/unable-to-find-a-cli-command-to-install-a-cursor-plugin-after-adding-its-marketplace-repository/166016). No settings-file route found. Ties to the ADR's "no CLI command" line at ADR:722; correct for install, stale for marketplace add.
- Auto-refresh: GitHub imports only, at most once per 10 min; existing entries only refreshed for individually-added plugins (docs). Confirms the ADR's "merge = publish" line (D11).

### 3. Public multi-harness repos

| Repo | Ships both marketplace files | Cursor per-plugin tree |
|---|---|---|
| grafana/ai-marketplace | yes (`.cursor-plugin/` + `.claude-plugin/` root manifests) | each plugin has `.cursor-plugin/plugin.json` **and** `.claude-plugin/plugin.json` (+ codex/grok variants) |
| stripe/ai | dirs `.claude-plugin`, `.cursor-plugin`, `.codex-plugin`, `.grok-plugin` seen; file bodies not readable via fetch | prior research: `.cursor-plugin/marketplace.json` -> `./providers/cursor/plugin/`, entry `version`/`author` |
| wshobson/agents | yes | `.cursor-plugin/` committed manifests; versions "kept in sync across the Cursor and Claude Code formats" |
| mike-north/ai-plugin-marketplace-template | yes, both GENERATED | per-target manifests generated |
| toolboxmd/marketplace | yes | separate `cursor/<pkg>/` tree, deliberately not named `plugins/` |

No repo found that ships **only** a root Agent Plugins `plugin.json` under a Cursor marketplace entry. Every working multi-harness repo carries a native `.cursor-plugin/plugin.json` per plugin. That is evidence of convention, not proof a root-only tree fails. Source: repos above, all 2026-09-29.

### 4. Settle by docs or live install?

Docs plus public examples settle: manifest lookup order, schema strictness, `./sub` source viability, dashboard/CLI add paths. A live install is needed for:

1. Does a marketplace entry whose dir has only root `plugin.json` (+ `skills/`, `mcp.json`) install with skills and MCP working? (Does folder discovery pick up `mcp.json`? Docs example in the issue text says `skills/`, `commands/`.)
2. With **both** `plugin.json` and `.cursor-plugin/plugin.json` present, which wins, and does either warn?
3. Does the runtime reject entry `version` and `1.4.0+<hash>`, or only the lint script?

Cheapest probe: a throwaway repo with three entries (root-only, both manifests, `.cursor-plugin`-only) imported through `cursor-agent plugin marketplace add file/git URL`, then `/plugin` list. Needs a Cursor login, which the owner must do. Alternative with no login: run the official `cursor/plugin-template` validator (`node scripts/validate-plugins.mjs`) on the exported tree; it settles item 3's lint half only.

### 5. Repo state read for this probe

- Agent Plugins tree today: `plugin.json` (with `$schema`, optional `extensions`), `mcp.json`, `skills/`, `README.md` (`src/export/family.rs:30-36, 214-250`; `stage.rs:1161-1178` writes `plugin.json` for `Family::AgentPlugins`). Cursor maps to `Family::AgentPlugins` (`family.rs:86, 355`).
- `ClaudeManifest` = `{name, version, description}` pretty-JSON + `\n` (`family.rs:47-53, 216-224`), a subset of Cursor's plugin schema.
- Stability: only the export report shape and the `<base>+<12-hex>` grammar are frozen; bytes inside an exported plugin are not (`docs/src/content/docs/stability.md:31-33, 195-204`).

## Recommendation

**(a) Does the Cursor tree need `.cursor-plugin/plugin.json`? Yes, treat it as required. Confidence ~80%.** The documented marketplace parse reads only that path, every working public repo ships it, and skipping it forfeits the manifest merge (manifest values win over the entry). A root-only tree would probably still load skills by folder discovery, but that is unproven and MCP loading is the doubtful part. Emit it; confirm with the live probe before the default set changes.

**(b) Emit it from `grim export plugin --client cursor` too: yes, but not with the same bytes.**
- Cursor's `plugin.schema.json` forbids `$schema` and `extensions`, so a copy of the Agent Plugins `plugin.json` breaks Cursor's own lint. Amend D2's "same bytes" to "the Claude-shape manifest bytes": reuse `family::claude_plugin_json` (already `{name, version, description}`, valid Cursor manifest). Cursor `logo` is a legal field if a logo is later wanted (additive, out of scope).
- Byte-equality with `export plugin --client cursor` holds only if both commands emit both files (one code path in `stage.rs`, so this is one added write, not a second renderer). D7's version rewrite must patch both manifests (step 4 rewrites `plugin.json`; add `.cursor-plugin/plugin.json`). The hash then covers both files, with each `version` excluded.
- Additive under the freeze: yes. Principle 9 allows new files/optional fields; `stability.md:195-204` names in-plugin bytes as unstable; the report shape and version grammar do not change. One visible effect: Cursor exports get a new `+<12-hex>` suffix once (same one-time re-version D7 already announces for every export), so ship it in the **same release as D7** rather than a later one that re-versions Cursor again. Add a release note; add an upgrade-fixture row.
- Keeping both manifests in one Cursor tree is inside Cursor's own submission checklist ("root `plugin.json` or `.cursor-plugin/plugin.json`"); precedence when both exist is an open live-probe item, and dropping root `plugin.json` for Cursor would break Agent Plugins byte-parity with Copilot/Codex trees for no proven gain, so keep both.

**(c) Default set: keep `cursor` out of the default set and make it explicit opt-in via `[marketplace].clients`; freeze that as the documented rule. Confidence ~65%.**
- Adding a default after 1.0 changes what every existing marketplace's next run writes (new `cursor/` trees, a new marketplace file, and Cursor auto-refresh reaching fleets on merge). An opt-in client never has that cost, and an explicit `clients = [..., "cursor"]` costs the curator one line.
- Three unproven items (entry `version` vs lint schema, `+build` version, dual manifests) sit exactly on the path a default would push onto every curator. Cursor's team-marketplace distribution is dashboard/interactive only, with no scriptable install, so a default gives the least verified channel the widest reach.
- If the owner prefers Cursor in the default set, do the live probe first and add it before 1.0, never after. Adding it later is the one move here that is a behaviour change.

**Also decide (not asked, found):** `${PLUGIN_ROOT}` and `${PLUGIN_DATA}` are not expanded by Cursor in `mcp.json`. The current Cursor tree ships them unexpanded (`family.rs:307`). Either decline plugin-root-relative MCP servers for `cursor` with a report row, or warn like `stage.rs:1101` does for other env refs. This is independent of the manifest question and affects `export plugin --client cursor` today.

## Open questions (need live Cursor or owner)

- Root-only vs dual-manifest behaviour and precedence (probe items 1-2).
- Does the runtime enforce the lint schema's `additionalProperties: false` on marketplace entries, so that grim's `version` is dropped or rejected? (Docs list `version`; lint schema does not; wshobson and stripe ship it.)
- Does `owner` need `email`? Docs: `owner.name` required, `email` optional; matches D4.
- What does Cursor do with `1.4.0+3f9a0c12b7de`: display, compare, or ignore?

## Sources

| Source | Type | Date | Relevance |
|--------|------|------|-----------|
| https://cursor.com/docs/plugins | Docs | 2026-09-29 | Agent vs Cursor plugin formats; team marketplace dashboard steps; auto-refresh |
| https://cursor.com/docs/reference/plugins | Docs | 2026-09-29 | manifest paths, marketplace fields, discovery steps, `${PLUGIN_ROOT}` not expanded, submission checklist |
| https://github.com/cursor/plugins (schemas/plugin.schema.json, marketplace.schema.json, .cursor-plugin/marketplace.json) | Repo | 2026-09-29 | strict schemas; official bare-`source` example |
| https://github.com/cursor/plugin-template | Repo | 2026-09-29 | per-plugin `.cursor-plugin/plugin.json` requirement; `validate-template.mjs` |
| https://forum.cursor.com/t/unable-to-find-a-cli-command-to-install-a-cursor-plugin-after-adding-its-marketplace-repository/166016 | Forum | 2026-07-19 | `cursor-agent plugin marketplace add`; no non-interactive install |
| https://forum.cursor.com/t/cursor-2-5-plugins/152124 | Forum | 2026 | Team Marketplaces (Cursor 2.6), local-import admin control |
| https://docs.withwillow.ai/docs/admin/install-marketplace/cursor | Third-party docs | 2026-09-29 | dashboard import, GitHub App prerequisite, per-plugin `.cursor-plugin/plugin.json` layout |
| https://github.com/orbi-build/orbi/issues/1437 | Issue | 2026-09-29 | quotes docs on `<source>/.cursor-plugin/plugin.json` + folder fallback |
| https://github.com/orbi-build/orbi/issues/1404 | Issue | 2026-09-29 | root Agent Plugins manifest claimed to list in Cursor Marketplace (claim, not test) |
| https://github.com/blindrelay-app/agent-plugin/pull/5 | PR | 2026-09-29 | Cursor GitHub importer drops `source: "."`; subdir source works |
| https://github.com/ArefMozafari/pr-evidence/issues/9 | Issue | 2026-09-29 | `source: "./"` not found on app import; subdir sources work |
| https://github.com/svennijhuis/agentPacks/pull/30 | PR | 2026-09-29 | "official-schema" catalog: bare sources, no extra entry fields |
| https://github.com/grafana/ai-marketplace | Repo | 2026-09-29 | both marketplace files; per-plugin `.cursor-plugin/plugin.json` beside `.claude-plugin/plugin.json` |
| https://raw.githubusercontent.com/wshobson/agents/main/.cursor-plugin/marketplace.json | Repo | 2026-09-29 | local-path sources with entry `version` in a working file |
| https://github.com/toolboxmd/marketplace, https://github.com/stripe/ai, https://github.com/mike-north/ai-plugin-marketplace-template | Repos | 2026-09-29 | multi-harness layouts |
| `.agents/research/research_marketplace_manifest_schemas.md` (lines 27-38, 47, 81-89) | prior research | 2026-09 | lint-vs-docs split; stripe `./providers/cursor/plugin/` |
| `.agents/research/research_plugin_support_matrix.md:48`, `research_agent_plugins_spec_verify.md:33` | prior research | 2026-09 | Cursor reads root `plugin.json` with spec `$schema`; update = ref refresh |
| `src/export/family.rs:30-53, 86, 214-250, 307-308, 355`; `src/export/stage.rs:1101-1112, 1161-1178` | repo | 2026-09-29 | today's tree, manifest structs, placeholder rename |
| `docs/src/content/docs/stability.md:31-33, 195-204` | repo | 2026-09-29 | frozen vs unstable export surface |

## Durable search terms

`cursor-plugin plugin.json marketplace.json` · `cursor plugins reference "Agent Plugins" loads in Cursor` · `cursor/plugins schemas plugin.schema.json additionalProperties` · `cursor-agent plugin marketplace add` · `cursor team marketplace import from repo auto refresh 10 minutes` · `cursor marketplace source "./" importer drops` · `.cursor-plugin/plugin.json .claude-plugin/plugin.json same repo` · `cursor mcp.json PLUGIN_ROOT not expanded` · `cursor plugin-template validate-template.mjs` · `agent-plugins.org cursor client support`
