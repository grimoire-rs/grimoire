State: handed-off → architect · Updated: 2026-10-20
Ratified: 2026-10-20 → architect
Confidence: ratified by owner (explicit /goal directive to run /hex-architect on this artifact); research vintages 2026-10-20 (3 artifacts: consumers recon, static index layouts, static site search — measured on a synthetic corpus)
Participants: owner + hex-discuss; one session, 2026-10-20

# Discussion: a better index structure than the monolithic all.json

## Intent

Extend the catalog-at-scale layout (`.agents/adr/adr_catalog_at_scale.md`, Proposed) from the
client catalog to the whole index: `all.json`, `stats.json`, the indexer's `enrich.json` checkpoint
and the index site's search data. The indexer builds incrementally, fetching only the parts of the
previous generation it needs, and skips unchanged subtrees by hash.

Trigger: `enrich.json` is 99% base64 files (live: changelog 62%, contents 23%, readme 13%; ~84 KB
per package). It crosses the 16 MiB checkpoint limit (`grimoire-indexer/src/enrich/checkpoint.ts`)
near 200 packages of that profile, and would be ~1.7 GB at 20,000 (~84 KB per package). Build time also grows with N. The
site ships the catalog twice: every card as island props in `index.html`, then the whole
`all.json` on the first keystroke (15 s timeout, falling back to a substring filter).

## Decisions

- **One root, three planes** (owner, 2026-10-20): `c/root.json` names `catalog`, `stats` and
  `enrich`. Each plane is 256 hash buckets. Enrich files move to a content-addressed blob store
  `c/b/sha256/<2>/<d>`. The enrich plane is pipeline-private, not a contract. `all.json` and
  `stats.json` stay as frozen flat floors.
- **3-level root: root → plane index → shard** (owner, 2026-10-20). `c/root.json` (~400 B,
  mutable) names each plane index by digest. `c/p/sha256/<d>.json` (~20 KB, immutable) holds that
  plane's 256 shard digests. Hourly stats churn changes only the root and the stats plane, so a
  catalog-only client fetches nothing beyond the 400 B root. This revises the ADR's 2-level root.
- **Previous state: CI cache first, published site as fallback** (owner, 2026-10-20). Blobs are
  keyed by digest and verified on fetch. Parked alternative: a git "publish" branch as the state
  store. The owner likes it (it might also suit publishing) but it is too large a change for the
  current setup.
- **Change detection: input hash plus a revalidate window** (owner, 2026-10-20).
  `input = H(pointer bytes ‖ resolved manifest digest ‖ enricher version)`. A bucket is skipped when
  its pointer hash is unchanged and its window has not elapsed. Otherwise each package gets one HEAD
  probe, and a deep enrich runs only when its input hash moved.
- **The window is a knob, defaulting to the build cadence** (owner, 2026-10-20).
  `enrich.revalidate` defaults to the cron interval (1 h), today's release latency. A large index
  raises it, and buckets are spread so each build probes 1/k of them. Pointers carry no version, so
  a release never changes a pointer hash: the window is the worst-case release latency.
- **Site search: one compact file now, chunked on a trigger** (owner, 2026-10-20).
  - `index.html` carries 48 cards plus per-kind counts and no full island payload.
  - A content-addressed `c/q/sha256/<d>.json` holds only the search fields. It is loaded on the
    first keystroke or scroll, and search and browse share it (~0.2 KB gzip per entry, ~4 MB at 20,000).
  - The digest file name means a data change gives a new name, and the rebuilt `index.html` points
    at it.
  - Past a size trigger, the site switches to Pagefind (search) plus paged per-kind listings
    (browse). Ceiling of the one-file shape: ~50-100k entries.
  - SQLite/Parquet over HTTP Range is rejected: neither target host serves Range correctly.
  - Strongest counter: GitHub Pages serves gzip only, so a slow mobile link waits seconds on the
    first keystroke. The substring filter remains the fallback.
- **Retention: files referenced by the last 3 generations, plus anything written in the last 24 h**
  (owner, 2026-10-20).
  - Pages hosts replace the whole site per deploy, and GitHub Pages edge-caches for 10 min
    (`max-age=600`).
  - Without retention, a client holding the previous root gets a 404 and falls back to the full
    `all.json`: about one refresh in six on hourly deploys.
  - Each extra generation adds only its changed files (~100 KB per release), because blobs and
    unchanged shards are shared.
  - An old digest that still 404s falls back to `all.json`.
- **`/enrich.json` is written for one more indexer release as a downgrade seed, then dropped**
  (owner, 2026-10-20). It was never a read contract.
- **Outcome: amend the Proposed ADR in place** (owner, 2026-10-20): both
  `.agents/adr/adr_catalog_at_scale.md` and `.agents/specs/design_catalog_at_scale.md`, which are
  unpushed.

## Shape (for the architect)

```
<index>/
  all.json, stats.json            frozen flat floors, unchanged
  enrich.json                     written for one more indexer release, then dropped
  c/root.json                     mutable, ~400 B: { schema, generation, flat, planes{catalog,stats,enrich} → digest }
  c/p/sha256/<d>.json             plane index: 256 bucket → {sha256,size}
  c/s/sha256/<d>.json             shard: bucket of refs (sha256(ref)[0]) → record/entry digest
  c/r/sha256/<2>/<d>.json         catalog record (the all.json element)
  c/b/sha256/<2>/<d>              enrich blob: readme, changelog, contents, logo
  c/q/sha256/<d>.json             site search file (search fields only)
  c/retained.json                 pipeline-private retention ledger (ADR D16)
```

```jsonc
// enrich shard entry, keyed by ref
"ghcr.io/acme/foo": { "input": "sha256:…", "checked": "2026-10-20T03:00Z",
                      "data": {…}, "files": { "readme.md": "sha256:…", … } }   // blob refs, not base64
```

Indexer build:
1. Restore blobs from the CI cache, and fetch `c/root.json` from the site.
2. Skip any bucket whose pointer hash is unchanged and whose window has not elapsed.
3. Otherwise send one HEAD per package (pool of 8), and run a deep enrich only when its input hash
   moved.
4. Fetch missing blobs from the site by digest, and verify them.
5. Emit the new generation plus every file of the last 2 generations and anything under 24 h old.

## Touched files and interfaces

- **ADR and design doc:** `.agents/adr/adr_catalog_at_scale.md` and
  `.agents/specs/design_catalog_at_scale.md`, amended in place. Affected: D2 layout, D3 opt-in, D5
  sync, D16 retention, D17 indexer build, plus a new site-search decision.
- **`grimoire-indexer`:**
  - `src/enrich/` (checkpoint, worker pool, input hash, window);
  - `src/data/index.ts` (emit planes);
  - `src/renderer/astro/` (`pages/index.astro`, `components/Catalog.tsx`, `lib/search.ts`);
  - `src/config.ts` (`sync.layout`, `enrich.revalidate`);
  - the rendered `pages.yml` template (CI cache restore and save).
- **`grim`:** `src/catalog/index_source.rs` plus the sync sidecar. It reads the catalog plane; the
  stats plane is an open question below.
- **Contracts:**
  - unchanged: `all.json`, `stats.json`, `/p/<ns>/<name>/`;
  - new permanent client-read paths: `c/root.json`, `c/p/`, `c/s/`, `c/r/`;
  - pipeline-private: `c/b/`, the enrich plane, `c/retained.json`;
  - site-private: `c/q/`.

## Out of scope

- Write side: announce, pointer schema 1, publish.
- The git "publish" branch as state store (parked).
- Segment model and governance.
- Search ranking quality.
- Telemetry.
- grim-side search UX.

## Open questions

- [NEEDS CLARIFICATION: Does grim sync the stats plane, or keep reading flat `stats.json`?]
  Recommended: keep `stats.json` for grim in this ADR. Stats are small (~200 B per package), and
  the plane mainly serves the indexer's own seed.
- [NEEDS CLARIFICATION: GitHub Pages' 1 GB published-size limit vs per-package pages that embed
  README and changelog (~84 KB per package on the live profile, ~1.7 GB at 20,000)]
  Recommended: measure on the real index first. If confirmed, cap the changelog rendered on the
  page and link the full blob.
- [NEEDS CLARIFICATION: The site-search size trigger value] Recommended: 8 MB gzip for the search
  file, measured at build, with a build warning before the switch.
- Carried from `.agents/discussions/index-scale-rollout.md`: hosting and transport of the
  index, the oldest grim and extension that must keep working, and the real announce rate plus
  ETag behaviour.

## Verification

- **Equivalence property test.** Flat `all.json`, bulk sync, and a delta from any retained
  generation yield equal catalog entries.
- **Indexer incremental build:**
  - a rebuild with no release and no elapsed window runs zero deep enrichments and emits a
    byte-identical catalog plane;
  - one release changes exactly one record, one shard, one plane index and the root;
  - a stats-only change leaves the catalog plane digest unchanged.
- **Retention.** A client holding root g-2 resolves every file. g-3 gets a 404, then the
  `all.json` fallback.
- **Checkpoint scale.** A 20,000-entry synthetic build finishes inside the CI job limit and never
  builds a single checkpoint file over 16 MiB.
- **Site:**
  - `index.html` stays under 100 KB at 20,000;
  - search loads exactly one `c/q/` file;
  - the substring fallback works when that load fails.
- **Compatibility matrix.** Old grim × new index, new grim × old index, old indexer seed ×
  new site.

## Research

- `.agents/research/research_all_json_consumers.md` — record shape, the four consumers and their
  field projections, live byte share, churn without a release, frozen statements. Key finding: the
  site ships the catalog twice, and grim, cards and search each read a ~13-15 field projection.
- `.agents/research/research_static_index_layouts.md` — 15 ecosystems. Closest shape: conda CEP 16
  (one mutable name→hash index plus immutable per-name shards, opt-in). No sharded ecosystem
  offers full-catalog search from static files. No static host documents retention across a
  full-site replace.
- `.agents/research/research_static_site_search_at_scale.md` — static search at 100k,
  measured on a synthetic corpus.
  - Pagefind: ~1 MB cold per query, but a filter-only browse loads every chunk (≈0.43 MB per 1,000 docs).
  - Whole-index libraries break near 100k.
  - SQLite/Parquet over Range fails on both target hosts.

## Related

- `.agents/discussions/index-scale-rollout.md` (ratified 2026-10-17, the parent discussion)
- `.agents/adr/adr_catalog_at_scale.md`, `.agents/specs/design_catalog_at_scale.md` (Proposed)
- `.agents/research/research_catalog_sync_migrations.md`
