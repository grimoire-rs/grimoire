# Research: marketplace phase 2 — codebase recon

## Metadata

Date: 2026-09-28 · Lane: codebase recon (hex-discuss entry wave, `.agents/discussions/marketplace-phase-2.md`)
Question: how the index Pages site is built, which digests and caches exist, what `grim export plugin` emits, whether "commands" exist as a kind, and whether `/marketplace/` fits the site layout.
Sources: grimoire-duo `src/export/*`, `src/command/export.rs`, `src/oci/artifact_kind.rs`, `src/store/*`, `.agents/discussions/harness-native-marketplaces.md`, `.agents/adr/adr_harness_plugin_export.md`; grimoire-indexer `src/{cli,enrich,data,renderer,ci.ts}`, `templates/ci/*`; grimoire-index `.github/workflows/*`, `index.config.json`, `package.json`, `dist/`, `enrich/`; grimoire-components `.gitlab-ci.yml`; `gh run view` of grimoire-rs/index run 36444909088.

## Q1 Index site build pipeline

- The index data repo is `grimoire-index` (site https://index.grimoire.rs). `@grimoire-rs/indexer` renders its workflows from `index.config.json`; nobody edits them by hand (`pages.yml:1-19`; the README says "The workflows are generated"). `grimoire-indexer` renders a GitHub and a GitLab variant (`src/ci.ts:31`, `templates/ci/gitlab-*.yml`).
- Triggers: a push to main, an hourly cron `23 * * * *` (`pages.yml:31`), `workflow_dispatch`, and a daily `refresh.yml` cron `17 4 * * *` (`refresh.yml:15`) that dispatches `pages.yml`.
- `pages.yml` steps:
  1. `ratings` job: `npx --no grim-indexer ratings` (`:254`), then uploads the `grim-stats` artifact.
  2. `build` job: checkout, setup-node with the npm cache, `npm ci` (`:65`), configure-pages.
  3. Install grim: fetches the release tarball with curl and checks its sha256 (`:77`).
  4. `npm run enrich -- --seed` (`:106`). This step is best-effort: if it fails, the job warns and builds without READMEs.
  5. Seeds the `grim-stats` artifact and `stats.json` from the published site.
  6. `npm run build` (`:196`), which is `grim-indexer build`. `compileIndex` clears `dist/` and writes `all.json`, `index/`, `logos/` and `enrich.json` (`src/data/index.ts:167-178`). `buildSite` then runs Astro over `<outDir>/all.json` and writes `sitemap.xml` and `robots.txt` (`src/renderer/index.ts:860-900`).
  7. Uploads the pages artifact, then the `deploy` job runs deploy-pages (`:213`).
- Only enrich talks to the registries (`src/enrich/index.ts`). Per package it runs, in order:
  - `grim describe <ref>` (`:262`)
  - `fetch --description --digest-only` (`:266`)
  - `fetch --description`, only if the digest moved (`:277`)
  - `fetch <ref>` for the contents (`:301`)
- Timings from run 36444909088 (scheduled, 19 packages, 2026-09-28), about 1m42s in total:

  | Step | Time |
  |---|---|
  | ratings | ~18s |
  | `npm ci` | 6s |
  | install grim | 2s |
  | enrich | 38s |
  | build | 4s |
  | upload | 2s |
  | deploy | ~7s |

  The enrich log says "seeded 7 sidecar(s) … enriched 19/19".
- Enrich dominates. The loop is sequential (`src/enrich/index.ts:340-350`), and its comment reads `ponytail: sequential ... batch with a worker pool if an index ever grows into the hundreds`. Each package costs at least one `describe` per run. Extrapolating about 2s per package gives roughly 13 minutes for 400 packages. That figure is an extrapolation, not a measurement. Astro render time at 400 pages has not been measured.
- What survives between runs:
  - The npm cache from setup-node. There is no CI cache for enrich, `dist/` or grim.
  - The previous deploy, used as a checkpoint: `enrich --seed` fetches `<site>/enrich.json`, which holds the base64 sidecars (`src/cli/enrich.ts:24-50`, `src/enrich/checkpoint.ts:1-30`). The ratings data is seeded from the previous `stats.json` the same way.
  - Only 7 of 19 sidecars were seeded in the run above. The cause has not been investigated; `checkpoint.ts` `hydrateFiles` (`:76`) is the place to start.

## Q2 Digest knowledge and skip mechanisms

- The digest is known before rendering. `describe` reports the artifact `digest`, which is stored as `contentDigest` in each sidecar. The description companion's digest is stored as `descDigest`, taken from `--digest-only` (`src/enrich/index.ts:266-275,295-305`). `compileIndex` strips only `descDigest` (`src/data/index.ts:~150`), so `contentDigest` ships in `all.json`.
- Enrich skips some work:
  - It skips the description download when `descDigest` is unchanged.
  - It skips the contents fetch when `contentDigest` is unchanged.
  - It re-stamps `updated` only when the digest moved (`:317-325`).
- `describe` itself is never skipped (`:262`: "describe is cheap and runs every time").
- The site render never skips anything. Each run does `rmSync(outDir)` and then a full Astro build (`data/index.ts:167`). No early exit based on a hash of the inputs exists in grim, the indexer or grimoire-components. The indexer's GitLab variant was not checked.
- `grimoire-components` has no build pipeline; its CI only runs a setup smoke test and a publish dry-run.
- On the grim side, the marketplace lock stores a declaration hash per plugin, and `is_stale` decides whether to re-resolve (`src/export/resolve.rs:102`, `src/export/marketplace.rs:68,200`). That skips work at resolve time; it is not a render cache.

## Q3 `grim export plugin` output

- Clients map to three families (`src/export/family.rs:82-89`):

  | Family | Clients |
  |---|---|
  | Claude family | claude, droid, junie, openclaw |
  | Agent Plugins | copilot, codex, cursor, agents |
  | `NoPluginFormat` | everything else |

- The output tree is `<output>/<name>.<client>/`, or `<name>.<client>.zip` with `--zip`.
  - Skills and agents are rendered through `ClientTarget::materialize` at global scope (`src/export/stage.rs:~962-970`). They land in `skills/<name>/` and `agents/<name>.md`.
  - MCP servers land in `.mcp.json` for the Claude family and `mcp.json` for Agent Plugins (`stage.rs:1010-1015`).
  - The manifest is `.claude-plugin/plugin.json` for the Claude family, and a root `plugin.json` with `$schema` for Agent Plugins (`stage.rs:1160-1176`, `family.rs:219-250`).
  - The logo lands at `assets/logo.<ext>`.
- Admission rules (`family.rs:107-121`): rules and bundles never ship. Agent Plugins carries no agents. Otherwise a member ships when the vendor's `kind_support` allows its kind.
- The version is `<base>+<12 hex>` (`family.rs:124-138`).
  - The base is `--version`, else the declared version, else the annotation, else `0.0.0`.
  - The 12 hex are the start of a sha256 over the sorted lines `kind\temitted\tcontent_digest`.
- The output is byte-reproducible (ADR, `adr_harness_plugin_export.md:49,554`):
  - Zip entries use Stored, the DOS epoch, mode 0644 and sorted names, with no directory entries (`src/export/archive.rs:15-50`).
  - Members and MCP entries are sorted (`stage.rs:1017-1019`).
  - The export path never reads the clock.
- Nothing is cached per export or per render. `stage_locked_artifact` fetches and verifies each blob, then materializes it into a fresh tempdir (`src/install/installer.rs:2128-2148`). Nothing in `src/export/` references `BlobStore` (`$GRIM_HOME/blobs`); whether `access.fetch_blob` caches internally was not traced. The only state carried between runs is `marketplace.lock`.

## Q4 "Commands" as an artifact type or render target

- No. `ArtifactKind` has five variants: Skill, Rule, Agent, Bundle and Mcp (`src/oci/artifact_kind.rs:27-51`). The indexer's `PackageKind` has the same five (`grimoire-indexer/src/data/index.ts:15`).
- No renderer handles slash commands, prompt files, `commands/` or `prompts/` (`src/install/vendor_*.rs`).

## Q5 Site layout and `/marketplace/`

- The published site serves these paths:
  - `/` and `/p/<ns>/<name>/`
  - `/all.json`
  - `/index/<ns>/<pkg>/metadata.json`
  - `/logos/<ns>/<name>.<ext>`
  - `/stats.json` and `/enrich.json`
  - `/sitemap.xml` and `/robots.txt`
  - `/404.html`
  - `/_astro/`
- Public directories are layered in this order: `DEFAULT_PUBLIC_DIR`, then the index repo's `public/`, then `outDir` (`src/renderer/index.ts:715-717`).
- No `/marketplace` route exists. The only mention of "marketplace" is a VS Code Marketplace link on the landing page (`index.astro:23-26`).
- `compileIndex` runs `rmSync` on `outDir` (`data/index.ts:167`). Anything written under `/marketplace/` therefore has to be written after that clear, the way `stats.json` is (`renderer/index.ts:860-873`).

## leads:

- Split the enrich time: how much is `describe` and how much is contents fetching, on an unchanged index of about 400 packages? This needs a real timed run.
- Why did only 7 of 19 sidecars seed? See `src/enrich/checkpoint.ts:76-110`.
- How long does Astro take to render about 400 pages?
- Does `access.fetch_blob` reuse `$GRIM_HOME/blobs` during export staging?
- Who owns the `/marketplace/` output: the indexer's `outDir`, the index repo's `public/`, or a separate job?
- How do the seeding and caching in the GitLab CI variant (`templates/ci/gitlab-ci.yml`, `gitlab-enrich.sh`) differ from the GitHub variant?
