# Research: Harness-native marketplace projection — codebase recon

## Metadata

**Date:** 2026-09-15
**Domain:** packaging
**Triggered by:** /hex-discuss harness-native-marketplaces
**Expires:** 2027-03-15

## Direct Answer

The pieces exist but nothing is wired together. The index (`grimoire-index` +
`grimoire-indexer`) already carries most catalog-display metadata a
`marketplace.json` needs (title, description, version, license, keywords,
owner, ref) and already runs `grim` in CI (`enrich/`), but only to pull the
description companion (README/CHANGELOG/logo) — never the artifact's actual
skill/rule/agent files. No code anywhere (grim, indexer, or the three prior
research docs) lays out a `.claude-plugin/plugin.json` + `marketplace.json`
tree or a VS Code Copilot `.github/plugin.json`. `adr_render_layout_stability.md`
already designed (but deferred) an adjacent, different thing: grim *installing*
a Claude plugin render locally at install time. A CI-generated, grim-free
marketplace repo is a distinct, unbuilt capability that would reuse the index's
per-package JSON and grim's OCI fetch primitives, but needs a new unpack path
(today's `grim fetch` only serves single files or the description companion,
not a whole artifact tree to an arbitrary directory) and a new generator
(nothing in `indexer`'s renderer touches plugin/marketplace formats — its only
"marketplace" hit is a marketing link to the VS Code extension listing).

## Key findings

### Axis 1 — Data sufficiency

- The index wire schema (`IndexPackage` in
  `/home/mherwig/dev/grimoire/src/catalog/index_source.rs:47-92`) carries:
  `schema`, `name`, `kind`, `ref`, `description`, `repository`, `keywords`,
  `summary`, `deprecated`, `replaced_by`, `license`, `created`. The compiled
  `all.json` the indexer actually emits is richer — `title`, `summary`,
  `version`, `license`, `created`, `keywords`, `tags` (full version list),
  `deprecated`, `hasReadme`, `hasChangelog`, `logo`, `description`, `kind`,
  `name`, `owner{github,id}`, `ref`, `repository`, `schema`, `namespace`
  (`/home/mherwig/dev/grimoire-index/dist/all.json:1-45`, produced by
  `/home/mherwig/dev/grimoire-indexer/src/enrich/index.ts:76-90` `META_KEYS`
  + `SUPPORT_CHANNELS` at lines 97-115).
- OCI manifest annotations grim writes per artifact
  (`/home/mherwig/dev/grimoire/src/oci/annotations.rs`): standard
  `org.opencontainers.image.{title,description,version,licenses,source,
  revision,created}` plus `com.grimoire.compatibility` (line 226),
  `com.grimoire.deprecated` (241), `com.grimoire.replaced-by` (273),
  `com.grimoire.keywords` (287), `com.grimoire.summary` (290), and
  `com.grimoire.kind` (`artifact_kind.rs:17`). Repository-level support
  channels (`issues`, `chat`, `contact`, `security`) live on the description
  companion manifest under `com.grimoire.support.*`
  (`/home/mherwig/dev/grimoire/src/oci/description.rs:93-115`).
- **What a `marketplace.json`/`plugin.json` needs that is present**: name,
  description, version, author/owner, keywords, license, repository URL,
  homepage (`metadata.url`/`documentation` from `catalog/publish.toml:31-35`).
- **What is missing or would need derivation**: no per-artifact icon beyond
  the repo-level `logo.svg` (one logo per *repository*, not per skill/plugin
  component); no `commands`/`agents`/`hooks`/`mcpServers` component listing —
  a plugin.json enumerates *files inside the plugin*, which the index never
  records (it stores only a `ref`, not a file tree); no target-harness
  compatibility field beyond the free-text `compatibility` annotation, which
  is skill-only and not machine-parseable (`annotations.rs:226`, "free-text
  editor/runtime requirement… no other kind has the field"); bundle members
  are grim `kind`/`name` pairs (`BundleMember` in
  `/home/mherwig/dev/grimoire/src/oci/bundle.rs:40-49`), not a
  plugin-component taxonomy.

### Axis 2 — Content access

- Bytes live only in OCI blobs today. `grim fetch <ref>` (CLI,
  `/home/mherwig/dev/grimoire/src/command/fetch.rs:29-63`) can print the
  canonical document, a `--vendor <client>` projection, or one `--path <file>`
  — there is **no flag that unpacks a whole artifact tree to a directory**.
  The only full-tree unpack CLI has is `--description --out <dir>`
  (line 62-63), which is scoped to the description companion (README/
  CHANGELOG/logo), not the artifact's own files.
- The indexer's `enrich` step already proves a CI job can run `grim` against
  the registry: it calls `grim describe <ref>` and
  `grim fetch <ref> --description --digest-only` / `--description`
  (`/home/mherwig/dev/grimoire-indexer/src/enrich/index.ts:262-292`) and
  writes the companion's `readme.md`/`changelog.md`/`logo.svg` to
  `enrich/<namespace>/<name>/` — confirmed on disk at
  `/home/mherwig/dev/grimoire-index/enrich/github.com/grimoire-rs/grim-essentials/{readme.md,changelog.md,logo.svg,data.json}`.
  That is precedent for "CI job invokes grim, harvests files" but for the
  *wrong* payload (description companion, not the skill/rule/agent source).
- The actual full-tree materialization code path exists but is coupled to a
  grimoire *project*: the render/projection engine
  (`/home/mherwig/dev/grimoire/src/install/render.rs`) and the
  installer/materializer (`src/install/installer.rs`, `materializer.rs`) run
  against a resolved lockfile + install-state inside `grim add`/`grim
  install`, not as a standalone "fetch ref → lay out plain files in this
  directory" primitive. Building a marketplace generator would need either a
  new fetch-and-unpack primitive or driving a throwaway `grim init` +
  `grim add` + `grim install` per package inside CI (heavier, but reuses
  everything rather than duplicating unpack logic).
- `grim build` (`/home/mherwig/dev/grimoire/src/command/build.rs:1-11`) packs
  a *local* skill/rule directory for publish — it is the inverse direction
  (local → OCI), not usable for registry → local unpack.

### Axis 3 — Kind coverage

- Grim kinds: `Skill`, `Rule`, `Agent`, `Bundle`, `Mcp`
  (`/home/mherwig/dev/grimoire/src/oci/artifact_kind.rs:27-53`).
- Claude Code plugin components (per
  `.agents/research/research_plugin_schemes.md:26` and
  `research_community_skill_packs.md:289-293`): skills, commands, agents,
  hooks, MCP servers. Mapping: `Skill`→skill (direct), `Agent`→agent
  (direct), `Mcp`→`.mcp.json` entry (direct — grim already models transport/
  command/url per `/home/mherwig/dev/grimoire/src/oci/mcp.rs:34-47`),
  `Bundle`→a plugin (one plugin per bundle, matching the deferred ADR's
  "one plugin per declared top-level unit",
  `adr_render_layout_stability.md:134-138`). **`Rule` has no Claude plugin
  target** — `adr_render_layout_stability.md:41-44,165-166`: "Claude's plugin
  format has no rules/memories surface… rules stay plain files no matter
  what." Confirmed independently by `research_plugin_schemes.md:41-44`.
- VS Code Copilot `plugin.json` (per `research_plugin_schemes.md:26-30,72`)
  supports commands/skills/agents/hooks/MCP in any combination — same rule
  gap: no dedicated rules surface, though Copilot agent mode does read
  `.claude/skills/`-style dirs via `chat.agentSkillsLocations`
  (`research_plugin_schemes.md:73,111`), which is a config-splice path
  distinct from a packaged plugin.
- Copilot CLI plugins are a lifecycle unit with no partial add/remove
  (`research_plugin_schemes.md:31-35,79`) — relevant if "marketplace" is read
  as "grim-free install," since that harness can't consume individual
  artifacts once bundled.

### Axis 4 — Where the generator would live

- Three plausible sites, with precedent for two:
  1. **Index-repo CI (indexbot pattern)** — has direct precedent: the
     generated workflows in `grimoire-index/.github/workflows/` are rendered
     from `index.config.json`'s `ci` block by `@grimoire-rs/indexer`
     (`/home/mherwig/dev/grimoire-index/README.md`, "The workflows are
     generated" section) and the `enrich` job already runs `grim` in CI.
     Adding a `marketplace` build step alongside `enrich`/`build` would sit
     naturally in `grimoire-indexer`'s `src/` (currently
     `cli/data/downloads/enrich/ratings/renderer/validate`, no
     `marketplace/` dir yet — confirmed via `find src -maxdepth 2 -type d`).
  2. **New `grim` subcommand** (`grim marketplace export` or similar) — no
     existing command does this; would need the axis-2 unpack primitive
     either way. Would let any registry owner (not just the grimoire-rs
     index) generate a marketplace from their own catalog, matching grim's
     "any OCI registry, no vendor lock-in" identity
     (`product-tech-strategy.md`, `adr_projection_over_index.md`'s general
     stance that grim resolves live rather than caching a second copy —
     though that ADR is about the TUI tree, not this).
  3. **Separate repo** (like `obra/superpowers-marketplace`, a distinct repo
     from the skill source, per `research_community_skill_packs.md:140-141`)
     — precedent exists in the community, not in grim's own tooling.
- No prior research or ADR picks one; `research_plugin_schemes.md:174-188`'s
  entire framing is *grim-installs-a-plugin-locally*, not *CI-publishes-a-
  marketplace-repo* — a different actor (grim binary vs. CI job) and a
  different consumer (grim user vs. harness-native, grim-free user).

### Axis 5 — Stability

- `adr_render_layout_stability.md` (Accepted, 2026-07-09) is the load-bearing
  prior decision: vendor render layout is explicitly **outside** the 1.0
  contract (`docs/src/content/docs/stability.md:131-150`, "Unstable — may
  change in any minor… Vendor render layout"), and Claude plugin rendering is
  reserved as a deferred, opt-in **mode on `ClaudeVendor`** (not a new
  `ClientTarget`), with sources in `$GRIM_HOME/claude/marketplace/…` /
  `<workspace>/.grimoire/claude/…` and registration through the existing
  `Vendor::sync_config` + `json_splice` seam
  (`adr_render_layout_stability.md:145-172`). A CI-generated public
  marketplace touches none of grim's frozen 1.0 surfaces (CLI args, exit
  codes, `--format json`, `grimoire.toml`/lock/state-V2 schemas —
  `stability.md:22-41`) since it would live entirely in the index repo /
  indexer, outside the grim binary — but it would consume the **unstable**
  `grim fetch`/`describe` output shapes and the **explicitly-not-a-contract**
  render layout if it tried to reuse per-vendor projection code.
- `adr_vendor_support_tiers.md:95-121`'s three-class gap-response boundary is
  directly applicable if this ever becomes a grim *feature* rather than pure
  index-side tooling: a static, deterministic, uninstall-safe marketplace
  render is Class 2 ("compensating render… deterministic, removed on
  uninstall, inert if grim vanishes") and gated to Tier 1 clients
  (`claude`, `codex`, `opencode`, `copilot` — line 121); anything requiring
  grim to keep a process/plugin alive against a harness API is Class 3,
  refused outright (lines 103-106).
- Principle 9 (`AGENTS.md`) — additive-only schema evolution — would apply to
  any new index wire field this needs (e.g. a per-component file listing);
  `adr_default_provenance_and_support_channels.md:168-178` already
  establishes the precedent that `all.json`/`stats.json` join derived,
  indexer-side data without grim writing it, which is the shape a
  marketplace-manifest field would likely also take.

## negative:

- `/home/mherwig/dev/index` (`ocx-sh/index`) is a **different product** —
  the generic OCX package index (kubectl, bazel, helm, etc., under
  `p/<org>/<tool>.json`) — not the grimoire package index. Per existing
  memory (`project-ocx-indexbot-reference.md`), it is architecture
  precedent only, not the data source; the actual grimoire index is
  `/home/mherwig/dev/grimoire-index` (serves `index.grimoire.rs`). The task
  brief's framing of `/home/mherwig/dev/index` as "the public index repo"
  was imprecise — it holds a `p/grimoire/cli.json` entry (grim indexed as a
  *consumer tool* in OCX's own index) but nothing about grim-published
  skills/rules/agents.
- No occurrence of "marketplace" or "plugin.json" generation logic anywhere
  in `grimoire`, `grimoire-index`, or `grimoire-indexer` source — the sole
  "marketplace" hits are: (a) prose in three research docs and two ADRs
  describing the *deferred* idea, (b) a hardcoded link to the VS Code
  Marketplace listing for `grimoire-vscode` in
  `grimoire-indexer/src/renderer/astro/pages/index.astro:23-26`, unrelated
  to catalog projection.
- `grim fetch --description --out <dir>` looks at first glance like the
  unpack primitive needed, but it is hard-scoped to the description
  companion tag (`__grimoire`) — `is_description_manifest` in
  `/home/mherwig/dev/grimoire/src/oci/description.rs:49-51` explicitly
  excludes it from `ArtifactKind`, so it cannot be redirected at an
  artifact's own tag.
- `adr_projection_over_index.md` is **not** about the package index at all —
  it is a TUI-internal ADR about the tree-view widget's data model
  (`rows`/`filtered`/`marked` vs. virtual bundle-member nodes). It was in the
  requested ADR list but is irrelevant to this topic; included here so it is
  not silently dropped, but it contributes nothing to marketplace feasibility.

## leads:

- **Plugin-container spike (Claude local install mode)** — `research_plugin_
  schemes.md`'s own next step (lines 169-172) is a hands-on spike against
  live Claude Code to resolve `known_marketplaces.json`/`enabledPlugins`
  mechanics; that spike would directly inform the *format* half of a
  CI-generated marketplace even though the *actor* (grim binary vs. CI job)
  differs.
- **Full-artifact-tree fetch primitive** — whether `grim fetch` gains a
  `--out <dir>` for the artifact itself (mirroring `--description --out`) is
  the concrete missing piece for axis 2; worth its own design spec rather
  than folding into a marketplace ADR, since other consumers (e.g. air-gapped
  mirroring) would also want it.
- **Per-component metadata in the index wire schema** — a marketplace needs
  file-tree/component info the index doesn't carry; scoping exactly what
  `all.json` would need to add (and whether that's additive per Principle 9)
  is a narrower follow-up than the full marketplace question.
- **`research_promotion_positioning.md`'s "Unclaimed narratives"/competitive
  landscape sections** (lines 111-149) were not re-read in depth for this
  pass — worth checking whether "publish once, consume grim-free everywhere"
  is already staked out as positioning language, since it would bear on
  whether this is a promotion play or a pure engineering one.

## Sources

- `/home/mherwig/dev/grimoire/src/catalog/index_source.rs`
- `/home/mherwig/dev/grimoire/src/oci/annotations.rs`
- `/home/mherwig/dev/grimoire/src/oci/artifact_kind.rs`
- `/home/mherwig/dev/grimoire/src/oci/description.rs`
- `/home/mherwig/dev/grimoire/src/oci/bundle.rs`
- `/home/mherwig/dev/grimoire/src/oci/mcp.rs`
- `/home/mherwig/dev/grimoire/src/command/fetch.rs`
- `/home/mherwig/dev/grimoire/src/command/build.rs`
- `/home/mherwig/dev/grimoire/src/install/render.rs`
- `/home/mherwig/dev/grimoire/catalog/publish.toml`
- `/home/mherwig/dev/grimoire/catalog/README.md`
- `/home/mherwig/dev/grimoire/docs/src/content/docs/stability.md`
- `/home/mherwig/dev/grimoire/docs/src/content/docs/publishing.md`
- `/home/mherwig/dev/grimoire/docs/src/content/docs/browse.md`
- `/home/mherwig/dev/grimoire/.agents/adr/adr_render_layout_stability.md`
- `/home/mherwig/dev/grimoire/.agents/adr/adr_vendor_support_tiers.md`
- `/home/mherwig/dev/grimoire/.agents/adr/adr_default_provenance_and_support_channels.md`
- `/home/mherwig/dev/grimoire/.agents/adr/adr_projection_over_index.md`
- `/home/mherwig/dev/grimoire/.agents/adr/adr_client_compat_matrix.md`
- `/home/mherwig/dev/grimoire/.agents/research/research_plugin_schemes.md`
- `/home/mherwig/dev/grimoire/.agents/research/research_community_skill_packs.md`
- `/home/mherwig/dev/grimoire/.agents/research/research_promotion_positioning.md`
- `/home/mherwig/dev/grimoire/.agents/discussions/harness-native-marketplaces.md`
- `/home/mherwig/dev/grimoire-index/README.md`
- `/home/mherwig/dev/grimoire-index/dist/all.json`
- `/home/mherwig/dev/grimoire-index/enrich/github.com/grimoire-rs/grim-essentials/data.json`
- `/home/mherwig/dev/grimoire-indexer/src/enrich/index.ts`
- `/home/mherwig/dev/grimoire-indexer/src/renderer/astro/pages/index.astro`
- `/home/mherwig/dev/index/README.md`, `catalog.config.json`, `package.json`
