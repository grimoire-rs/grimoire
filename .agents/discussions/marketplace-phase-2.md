# Discussion: Marketplace phase 2 — hosted harness marketplaces, commands, render speed

State: handed-off → architect · Updated: 2026-09-28
Ratified: 2026-09-28 → architect
Confidence: ratified by the owner (explicit "yes hand off" after the six-part restate and a layout walkthrough); research vintage 2026-09-28 (recon, hosting prior art, incremental Pages builds, URL trust, multi-harness root with local probes, multi-version)

## Intent

Phase 1 of plugin export landed (`grim export plugin`, `marketplace.toml` /
`marketplace.lock`, `strip_prefix`, per-client placeholder translation;
released v0.14.3). Phase 2 is how grim supports harness marketplaces. Owner's
wish state: alongside the package index, a `/marketplace` that every
supported harness can add, where each harness's marketplace file points at
plugin trees rendered for that harness. Also in scope: whether a marketplace
can list anything but plugins, whether "commands" need support, and render
time — the owner's index carries 400+ artifacts, built on GitLab Pages
(GitHub Pages equally possible), and a marketplace multiplies the output.
Owner's idea: a hash state derived from what would be rendered, to skip
work when the site is already up to date.

Out of scope: commands (deferred, Decisions); a Pages-hosted URL
marketplace; multiple versions of one plugin in one marketplace or release
channels; marketplace git tags for consumer pinning; Gemini, Kilo, Kiro
targets; any new artifact kind; changes to the frozen index format.

Prior discussion and settled phase-1/phase-2 decisions:
`.agents/discussions/harness-native-marketplaces.md` (handed-off → loop,
2026-09-27); ADR `.agents/adr/adr_harness_plugin_export.md`.

## Requirements

Two work streams; the grim one is gated by an ADR first.

- grim: an ADR settles `grim export marketplace` before code — CLI shape,
  `--format json` output, exit codes, marketplace-level metadata in
  `marketplace.toml`, the on-disk repo layout, and the plugin-version hash
  change below — against `subsystem-cli-api.md`, `quality-rust-exit_codes.md`
  and Principle 9 (additive only).
- grim: `grim export marketplace` writes one marketplace repo tree: per
  client a marketplace file at that harness's own path and a rendered
  plugin tree under `./<client>/<plugin>/`, rendered exactly like
  `grim export plugin --client <client>` (reuse mandate of 2026-09-27
  applies: no second render, lock or update path).
- grim: first-cut clients Claude Code (`.claude-plugin/marketplace.json`),
  Copilot (`.github/plugin/marketplace.json`), Codex
  (`.agents/plugins/marketplace.json`), Cursor
  (`.cursor-plugin/marketplace.json`); Droid (`.factory-plugin/`) and Junie
  (`.junie-extension/`) in the same cut if the ADR finds the effort small —
  both are already Claude-family render targets in phase 1.
- grim: every `source` path is repo-root-relative (`./<client>/<plugin>`),
  never `../`; plugin dir names are free, entry `name` equals manifest
  `name`.
- grim: incremental export — a plugin whose skip key (grim version, resolved
  member digests, declaration hash) matches the committed tree is not
  re-fetched or re-rendered; a grim version change forces a full re-render;
  an unchanged run leaves the tree byte-identical (no diff = no commit).
- grim: the plugin `version` suffix hashes the rendered plugin tree, so a
  renderer-only byte change yields a new version and harnesses offer the
  update; format stays `<base>+<12 hex>`.
- grim: an entry dropped from `marketplace.toml` disappears from every
  marketplace file and its tree is deleted.
- Marketplace repo: scheduled job from a `grimoire-components` component —
  `grim update --marketplace` → `grim export marketplace` → PR on diff
  (decided 2026-09-27).
- Indexer (`grimoire-indexer`): enrich replaces the unconditional
  `grim describe` with HEAD digest probes diffed against the seeded
  `enrich.json`, runs per-package work in a bounded worker pool, keeps the
  daily `refresh.yml` full describe as the backstop for tag-list changes,
  and renders a `/marketplace` landing page written after `compileIndex`'s
  `rmSync`. Same change for the GitHub and GitLab CI variants.
- Docs pages, acceptance tests and the `grim-usage` catalog drift review
  land with the grim feature (`catalog/README.md`).

## Decisions

- Scope: the marketplace stays a **curated view** (hand-picked seed, grown by
  hand), not a mirror of every index bundle — reaffirmed by the owner
  2026-09-28, unchanged from 2026-09-27.
- Entry kinds: **plugins only**. Every targeted harness catalog lists plugins
  only (Kilo, not a target, is the exception); a lone skill ships as a
  one-skill plugin. Confirmed again by `research_marketplace_hosting_prior_art.md`.
- Commands: **deferred** (owner, 2026-09-28, after research). Skills are
  already slash-invocable in Claude, Copilot, Cursor, Droid; Codex plugins
  carry no command component; no upstream documents a command invoking a
  skill. Revisit trigger: a target family without per-skill invocation but
  with a command component (Gemini `commands/*.toml`) enters scope.
- Render speed: **both fixes, indexer first** (owner, 2026-09-28). Indexer
  (`grimoire-indexer`) — the measured bottleneck is enrich's sequential
  per-package `grim describe`; marketplace export (grim) — skip a plugin
  whose derived version matches the deployed `marketplace.json`. Two repos,
  two work streams. Pages deploys are full uploads on both forges; skipping
  saves compute, never upload.
- Removal: an entry dropped from the curated set is **removed** on the next
  export, no deprecated tombstone (owner, 2026-09-28; closes the
  2026-09-27 open question). Claude keeps installed copies unless the admin
  sets `forceRemoveDeletedPlugins`.

- Hosting: a **separate git marketplace repo** (public:
  `grimoire-rs/marketplace` recommended; an internal GitLab index gets its
  own marketplace project), one marketplace file per harness at the repo
  root, each pointing at its own rendered tree; regenerated by the
  scheduled `grim update --marketplace` → `grim export marketplace` → PR job
  decided 2026-09-27. The index Pages site carries only a `/marketplace`
  landing page with per-harness add commands (owner, 2026-09-28). Why: git
  is the only transport every targeted harness accepts (Copilot, Codex,
  Cursor have no URL add), the only one consumers can pin (`#ref`/`@sha`),
  and the committed trees are the persistent render state Pages cannot
  provide (no partial deploy). Rejected: Pages-URL-only (Claude family
  reach only, no pinning); inside the index repo (bot commits mixed with
  curation, marketplace name bound to the index).
- Pinning: **floating** entries (owner, 2026-09-28, conditional on the
  multi-version research, which came back negative). One marketplace
  serves one version per plugin (Claude rejects duplicate names, Codex
  first-wins); no install-time version selector in any harness. Consumers
  pin the whole marketplace by git ref; channels would be separate
  marketplaces. Consistent with "latest major only" (2026-09-27).
- Index enrich change probe (owner, 2026-09-28): the deployed
  `enrich.json` already carries each sidecar's `contentDigest` and
  `descDigest` (`grimoire-indexer/src/enrich/index.ts:262-305`); replace the
  unconditional `grim describe` with HEAD-only digest probes
  (`grim fetch <ref> --digest-only`, plus `--description`) diffed against
  that seed, and describe/fetch only what moved; run the per-package work
  in a bounded worker pool. Deprecation is a manifest annotation
  (`com.grimoire.deprecated`, `src/oci/annotations.rs:241`), so it moves the
  digest; the `tags` list does not (an old-line patch release) — the daily
  `refresh.yml` full describe stays as the backstop.
- Renderer drift (owner, 2026-09-28): the plugin `version` hash covers the
  **rendered plugin tree**, not the member digests, so a grim upgrade that
  changes bytes yields a new version (harnesses compare the version string)
  and an upgrade that changes nothing yields none. The pre-render skip key
  stays (grim version, member digests, declaration hash). Rejected: grim
  version in the hash (every release bumps every plugin); accept and
  document (renderer fixes never reach users until a member moves).
- Clients (owner, 2026-09-28): the four probed harnesses are the floor;
  Droid and Junie join the first cut if the ADR finds it cheap. Qoder is
  wanted near-term (large user base in China) — see Open questions.
- Write ownership (2026-09-28, walked through with the owner): only the
  marketplace repo's own CI writes to it, with a repo-scoped token
  (GitHub `GITHUB_TOKEN` `contents: write` + `pull-requests: write`, which
  needs the repo's "Allow GitHub Actions to create pull requests" setting;
  a GitLab project access token). The index CI never writes to the
  marketplace repo, so no cross-repo token exists. Triggers: schedule, plus
  push to main so a curator's merged `marketplace.toml` change exports
  without waiting. Illustrative layout (the ADR finalizes it):
  `marketplace.toml` + `marketplace.lock` at root; one marketplace file per
  harness (`.claude-plugin/`, `.github/plugin/`, `.agents/plugins/`,
  `.cursor-plugin/`, optionally `.factory-plugin/`, `.junie-extension/`),
  each listing `"source": "./<client>/<plugin>"`; one rendered tree per
  client under `./<client>/<plugin>/`; CI file including the
  `grimoire-components` component.
- Handoff (owner, 2026-09-28): ADR first (`/hex-architect`), then plan; the
  indexer change is its own small plan in `grimoire-indexer`.

## Research

- `.agents/research/research_marketplace_phase2_recon.md` (2026-09-28,
  codebase) — index site built by `grimoire-indexer`-generated CI; enrich
  (sequential `grim describe` + fetch per package) dominates: 38s of ~1m42s
  at 19 packages, extrapolated ~13 min at 400 (not measured); `contentDigest`
  known per artifact and shipped in `all.json`; enrich already skips fetches
  on unchanged digest but always runs `describe`; site render is a full
  `rmSync` + Astro rebuild; `grim export plugin` is byte-reproducible,
  versioned `<base>+<12 hex over member digests>`, and has no render cache;
  no "commands" kind exists; `/marketplace/` collides with no route but must
  be written after `compileIndex`'s `rmSync`.
- `.agents/research/research_marketplace_hosting_prior_art.md` (2026-09-28,
  web) — marketplace files: Claude `.claude-plugin/` only; Copilot
  `.github/plugin/` → `.claude-plugin/`; Codex `.agents/plugins/` →
  `.claude-plugin/`; Droid `.factory-plugin/` → `.claude-plugin/`; Junie
  `.junie-extension/` or `.claude-plugin/`; Cursor `.cursor-plugin/`; Agent
  Plugins 1.0 defines none. Plain-URL marketplace add documented only for
  Claude, Junie, Qoder, Droid; Claude requires absolute plugin sources
  (`archive` zip+sha256, `url`, `github`, `git-subdir`, `npm`) for a
  URL-added marketplace. Dir name free; entry `name` must equal manifest
  `name` (Claude). Skills slash-invocable in Claude, Copilot, Cursor, Droid;
  not per-skill in Codex, Gemini; no source documents a command invoking a
  skill.
- `.agents/research/research_incremental_pages_builds.md` (2026-09-28, web +
  code) — no partial deploy on GitHub or GitLab Pages; CI caches unreliable
  (GitHub 7-day eviction, GitLab "not guaranteed"); Astro experimental
  `incrementalBuild` skips pages by `cacheKey` but is disabled by
  `build.concurrency > 1`; grim `describe`/`fetch` take one ref per call
  (~3 requests per describe, ~800 process spawns at 400 artifacts);
  `grim fetch --digest-only` is one HEAD; caveat: `describe` also refreshes
  tags, support links, deprecation — a digest-only probe misses those.
- `.agents/research/research_url_marketplace_trust.md` (2026-09-28, web) — no
  harness pins a URL-hosted catalog; `#ref`/`@sha` pins exist only for git
  marketplaces (Claude, Droid, Codex); Claude `archive` `sha256` optional,
  enforced when set; Plugin4Shell (Sept 2026) — git SHA-pin verification
  missing in several CLIs, fixed in Claude Code 2.1.179 / Codex 0.146.0;
  Claude keeps an installed plugin whose entry disappears unless
  `forceRemoveDeletedPlugins`; Pages custom-domain takeover window
  documented on both forges; admin allowlists match the marketplace
  source, not entries.
- `.agents/research/research_marketplace_multi_harness_root.md` (2026-09-28,
  source + isolated local probes) — one repo root carrying
  `.claude-plugin/`, `.github/plugin/`, `.agents/plugins/` marketplace files,
  same marketplace name, each pointing at its own tree: Claude Code, Copilot
  CLI 1.0.88 and Codex each read only their own file and installed their
  own tree (probed). First hit wins (Codex `marketplace.rs:20-25`
  `find_map`; Copilot probe order root → `.plugin/` → `.github/plugin/` →
  `.claude-plugin/`; invalid primary errors, no fallback). Sources relative
  to repo root (`./claude/foo`); `../` rejected. Precedent: `stripe/ai`
  (per-harness files → `providers/<harness>/plugin/`), `grafana/ai-marketplace`.
  Caveat: openai/codex#19372 reports Codex auto-importing `.claude-plugin`
  marketplaces. Droid, Junie, Cursor unprobed.
- `.agents/research/research_marketplace_multi_version.md` (2026-09-28) —
  one version per plugin per marketplace (Claude `plugin validate` rejects a
  duplicate name; Codex first entry wins silently); no install-time version
  selector anywhere; per-entry `ref`/`sha` pins on git sources everywhere
  but Cursor; Claude release channels = two differently-named marketplaces;
  marketplace-level pin: Claude `#ref` branch/tag only (update follows the
  ref), Codex `--ref` full SHA, Droid `#ref`/`@sha`.

## Open questions

- [NEEDS CLARIFICATION: Qoder — plugin and marketplace format; phase 1 maps
  Qoder to no plugin family (`src/export/family.rs:82-89`)] Recommended: the
  ADR's research phase verifies Qoder's plugin manifest and marketplace
  file; include it in this cut if it matches an existing family, else a
  follow-up family plan.
- [NEEDS CLARIFICATION: version-hash scope — rendered-tree hash for
  `grim export marketplace` only, or also `grim export plugin`?]
  Recommended: both, one derivation; the format is unchanged and the value
  moves once on upgrade — the ADR confirms this against Principle 9 and the
  phase-1 ADR (`.agents/adr/adr_harness_plugin_export.md`).
- [NEEDS CLARIFICATION: Codex auto-importing `.claude-plugin` marketplaces
  (openai/codex#19372) — does a Codex user see the Claude tree next to the
  Codex one?] Recommended: smoke-test against the current Codex release in
  the plan's verification; if it double-lists, document it rather than
  drop the Claude file.
- (carried from 2026-09-27) [NEEDS CLARIFICATION: publisher opt-in
  annotation value semantics and where candidates surface] Recommended:
  as recorded in `.agents/discussions/harness-native-marketplaces.md` ›
  Open questions — value `true`, `grim export marketplace --candidates`.
- [NEEDS CLARIFICATION: `grim export marketplace` JSON output schema and
  exit codes] Recommended: settle in the ADR, mirroring
  `grim export plugin`'s report (written paths per client, skipped
  plugins, removed plugins, version per plugin).
- [NEEDS CLARIFICATION: marketplace-level metadata — name, owner,
  description, homepage per marketplace file] Recommended: a
  `[marketplace]` table in `marketplace.toml`, additive; the ADR maps it to
  each harness's required fields.

## Verification

- `grim export marketplace` acceptance tests under `test/`: a fixture
  `marketplace.toml` yields each first-cut client's marketplace file at its
  own path with `./<client>/<plugin>` sources and no `../`; each plugin
  tree byte-equal to `grim export plugin --client <client>` for the same
  plugin.
- Incremental: a second run with no upstream change writes nothing (tree
  hash identical, no files touched); a member bump re-renders only that
  plugin; a simulated grim-version change re-renders all; a removed
  declaration deletes the tree and every entry.
- Version: a renderer-only byte change produces a new `<base>+<12 hex>`
  version; an unchanged render keeps it.
- Harness smoke (manual or scripted with throwaway config dirs, as in
  `research_marketplace_multi_harness_root.md`): add the generated repo in
  Claude Code, Copilot CLI and Codex; each lists and installs only its own
  tree.
- JSON output and exit codes per `subsystem-cli-api.md` and
  `quality-rust-exit_codes.md`; `task verify`; `task catalog:verify` after
  the `grim-usage` drift review; docs page passes `task docs:check`.
- Indexer: at an unchanged index, enrich issues only HEAD probes (no
  `describe`, no fetch); a timed run on the ~400-artifact GitLab index
  before and after; the landing page survives `compileIndex`.

## Related

- `.agents/discussions/harness-native-marketplaces.md` — phase 1 and the
  recorded phase-2 design (curation, maintenance job, reuse mandate).
- `.agents/adr/adr_harness_plugin_export.md` — phase-1 ADR (version
  derivation, reproducibility, `marketplace.lock`).
- `.agents/plans/plan_harness_plugin_export.md` — landed phase-1 plan.
- `src/export/family.rs`, `src/export/stage.rs`, `src/export/archive.rs`,
  `src/command/export.rs` — phase-1 export seams.
- `grimoire-indexer/src/enrich/index.ts`, `src/enrich/checkpoint.ts`,
  `src/data/index.ts`, `templates/ci/` — indexer seams.
