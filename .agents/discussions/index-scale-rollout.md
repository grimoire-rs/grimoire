State: handed-off → architect · Updated: 2026-10-17
Ratified: 2026-10-17 → architect
Confidence: ratified by owner; research vintages 2026-10-17 (4 research artifacts + 3-seat council), synthetic benchmark 2026-10-17
Participants: owner + hex-discuss; one session, 2026-10-17

# Discussion: Index and catalog for large indexes

## Intent

Indexes with more than 500 entries; design point 20,000 entries, 100,000
ceiling. Owner's framing: decide first what we are trying to
achieve before planning fixes. Outcome: an ADR, drained to `/hex-architect`
(`high` floor).

Goal: a large catalog where people can **find** the right artifact,
**trust** it (owner, support, maintained), and the platform can **see** what is
used. This ADR serves the lead goal, **robust at scale**: grim, the indexer
and the VS Code extension do not truncate, time out or fail at the 20,000-entry design point
across many clients, and every grim / indexer / extension version mix
keeps working. grim is public OSS — the outcome must make sense for generic
users.

Why now: indexes approaching or past grim's former 500-entry cap
(`MAX_CATALOG_REPOS`, `src/catalog/registry_catalog.rs:50`); past it, rows are
silently unreachable.

Out of scope: index site rendering performance (in flight elsewhere); segment
model (one index + filters vs several — governance ADR); index governance
guide; metadata quality gate; usage telemetry / OpenTelemetry; search ranking
quality, keyword/owner fields in search output, duplicate detection (later
findability ADR).

## Requirements

- `/all.json` stays served, byte-shape unchanged, for old clients — frozen
  contract (`docs/src/stability.md:40`, `grimoire-indexer/docs/reference/output-layout.md`).
- Rolling release: one index serves both `/all.json` and the new layout;
  new grim uses the new layout when present and falls back otherwise; old grim
  never sees the new paths. Any version mix of grim × indexer × extension works.
- Additive-only under the 1.0 freeze (AGENTS.md Principle 9): new JSON fields
  optional/always-present per the null policy, zero new exit codes preferred,
  every new published path is permanent once shipped.
- Index sources are not capped at 500; the cap stays for the OCI `_catalog`
  walk only. Browse filters, query, sort, `grim rate` and `grim status --check`
  reach every row.
- Transport: `Accept-Encoding` negotiation with a statically linked decoder,
  plain JSON accepted; a decoded-byte body cap; connect + stall timeouts
  instead of the 30 s total (`src/catalog/index_source.rs:34`); a failed
  refresh serves the cached catalog instead of erroring
  (`registry_catalog.rs:648,685`); conditional GET with a stored validator.
- Clients at 20,000 rows: additive `total` / `truncated` and an opt-in limit in
  `grim search --format json` and the MCP `grim_search` tool, applied after
  filter, query and rank; the extension neither overflows its buffer
  (`grimoire-vscode/src/grim.ts:654`, 16 MiB) nor renders 20,000 cards at once.
- Write side unchanged: `grim publish --announce` keeps writing only the
  versionless pointer `index/<host>/<ns>/<pkg>/metadata.json` by PR/MR
  (`src/catalog/index_announce.rs`) and never reads the served layout, so no
  publisher, CI recipe or announce flag changes with the layout. Served files
  (`/all.json`, the Merkle tree) are build output of the index's own pipeline.
  The Merkle tree must never be committed to the index repo: every announce
  PR would then rewrite `root.json` and its shard, so concurrent announces
  would conflict and serialize. The announce PR contract is the pointer path
  plus pointer `schema: 1`; an upgraded index keeps accepting schema-1
  pointers from old grim indefinitely (the gate rejects any other schema
  today, `grimoire-indexer/src/validate/core/metadata.ts:87`), and new grim
  keeps writing schema 1. Announce does a `--depth` clone
  (`src/catalog/index_announce.rs:237`), so a 20,000-pointer repo stays cheap.
- Index upgrade is an operator action, reversible: bump the pinned
  `@grimoire-rs/indexer` in the index repo's lockfile, opt in through
  `index.config.json` (key named by the ADR), re-render CI (`npm run ci`). The
  next deploy serves `/all.json` plus the Merkle tree. Opting out removes the
  tree; new clients then 404 on the probe and fall back to `/all.json`.
- Content-addressed files survive a deploy: GitHub/GitLab Pages replace the
  whole site per deploy, so the build must carry the previous generation(s)
  forward — the same seed-from-last-deploy mechanism `pages.yml` already uses
  for `enrich.json` and `stats.json` (`grimoire-index/.github/workflows/pages.yml`).
- Indexer builds 20,000 entries within the CI job limit: the enrich checkpoint
  limit (16 MiB, `grimoire-indexer/src/enrich/checkpoint.ts:150`) and
  sequential enrich are resolved.

## Decisions

- **Lead goal: robust at scale**; findability, trust and usage evidence are
  separate later decisions (owner, 2026-10-17).
- **Outcome: ADR** "catalog at scale" covering the cap, transport, index
  layout and sync, indexer build scalability, and client robustness at 20,000
  rows. Amend or supersede the parked
  `.agents/adr/adr_catalog_freshness_revalidation.md` (Proposed) rather than
  writing a parallel record — the architect decides which (owner, 2026-10-17).
- **Pre-500 fixes ship ahead of the ADR as a bugfix**: cap removal for index
  sources, VS Code `maxBuffer` (shipped first), gzip + body cap, connect +
  stall timeouts, stale-cache fallback, plus the false "narrow the query"
  warning (`src/command/search.rs:176`) and every doc stating 500. All
  additive and needed under any layout; the ADR records them as given
  (owner, 2026-10-17).
- **Merkle layout: design now, ship on trigger.** Triggers: ETag churn on the
  host across a no-op redeploy, most hourly polls seeing an `all.json` change,
  egress approaching the host's cap (owner, 2026-10-17).
- **Root shape: two-level hashed.** Only `root.json` is mutable (~256 shard
  digests, ~20 KB). Shards are buckets keyed by `sha256(name)` prefix (~78
  names, ~8 KB), content-addressed and immutable; per-package records are
  content-addressed and immutable. A release costs a client ~30 KB vs ~870 KB
  (flat map, ~43 B per entry) vs ~7 MB gzip (`all.json`, ~0.33 KB gzip per
  entry), and only the 20 KB root is re-fetched
  if the host's ETags churn. Hash buckets keep segment names out of the
  contract. Cold start bootstraps from `/all.json` once. Illustrative paths
  (the ADR fixes them): `/c/root.json`, `/c/s/sha256/<d>.json`,
  `/r/sha256/<d>.json` (owner, 2026-10-17).
- **Discovery: probe the sibling path.** New grim tries `<dir of the
  configured all.json>/c/root.json`; present → Merkle sync, 404 → `/all.json`
  with the miss cached for a TTL. No config change on the fleet (owner,
  2026-10-17).
- **Compression is negotiated, not precompressed.** grim sends
  `Accept-Encoding` and decodes whatever the server chose (reqwest `gzip`,
  pure-Rust `flate2`, statically linked); plain JSON is fine. The indexer
  emits no `.gz`/`.br` siblings — GitLab Pages serves them only for
  htm/html/xml/txt/js/css/svg, not `.json`. On a host that never compresses
  JSON, `/all.json` travels raw (~2 KB per entry, ~40 MB at 20,000); the small Merkle files are
  what make such hosts viable (owner, 2026-10-17).
- **Segment model out of this ADR**: with the cap gone both models work
  (filters match the ref path, not the index namespace); the choice is
  governance (owner, 2026-10-17).
- **Success criteria** (owner, 2026-10-17), on a 20,000-entry index:
  - no truncation in search, `rate`, `status --check` or browse filters;
  - warm `grim search` under 1 s and under 300 MB RSS;
  - under 100 KB transferred per client per release once Merkle is live;
  - no failed refresh on a slow link — the cached catalog is served;
  - the indexer builds 20,000 entries within the CI job limit;
  - old grim and old extension against a new index, and new grim against an
    old index, all keep working.

## Threads

- Closed: cap, layout, root shape, discovery, compression, segment scope,
  success criteria — see Decisions.
- For the architect: generation retention for content-addressed files (an
  old root must not 404 — keep N generations, fall back to `/all.json` on any
  404 or digest mismatch); record shape (full record vs pointer + enrich);
  whether `stats.json` joins the Merkle tree; a `grim` diagnostic showing
  generation and sync state for support desks; old-client `/all.json` load
  (only a CDN removes it); merging the indexer's unmerged enrich worker-pool
  branch vs fresh work; whether root signing is reserved now. Note: pointers
  are versionless, so a new release of an existing package never touches the
  index repo — record churn comes from enrich (`version`, `tags`, `updated`)
  at rebuild time; the rebuild schedule, not the announce rate, sets the
  change frequency.

## Research

- `.agents/research/research_index_findability_change_rate.md` — search/rank,
  TUI and extension behaviour at scale, duplicates, change rate: a rebuild
  without releases changes no `all.json` bytes; every announce triggers one.
- `.agents/research/research_internal_catalog_success_at_scale.md` — internal
  catalogs at 10k+: success metrics, duplication (~46% shared names in a
  public skills catalog), catalog rot, Helm's single `index.yaml` OOM at
  13.4 MB.
- `.agents/research/research_index_scale_archaeology.md` — the freshness ADR
  was parked in 29ded5af with no recorded reason; none of its transport parts
  exist in code. The indexer 16 MiB limit was reused from a ratings buffer.
  Unmerged branch `feat/marketplace-enrich-landing` has an enrich worker pool
  (concurrency 8), digest-probe carry-forward and a rotating re-describe slice.
- `.agents/research/research_catalog_sync_migrations.md` — Cargo (root
  `config.json`, git index served indefinitely), PyPI (content negotiation —
  impossible on GitHub Pages), APT by-hash (advertised in the root, fallback
  to canonical paths), npm (10-week dual-serving, forced cutover, reported
  404s). No documented content-derived ETag on either Pages host.
- Council on index layout (premortem, operability, simplicity; inline,
  2026-10-17): all three require gzip + conditional GET first and reject
  namespace-sharded feeds (frozen shard key, hot shards). Premortem and
  operability make the digest layout the destination, operability as a
  two-level root; simplicity holds it until measured — reconciled by "design
  now, ship on trigger". Arithmetic, unmeasured: every 1,000 machines polling
  hourly without 304s pull ~108 GB/month even for a ~150 KB gzip catalog, already
  past GitHub Pages' 100 GB/month soft cap.
- Earlier session data (synthetic benchmark, uncapped build): warm and cold
  search both under 0.4 s; RSS ~5.6x the raw `all.json`; `all.json` ~2 KB raw
  and ~0.33 KB gzip per entry. Not persisted in the repo; re-run against a
  real index.

## Related

- `.agents/adr/adr_catalog_freshness_revalidation.md` (Proposed — conditional
  GET, body cap, stale fallback)
- `.agents/adr/adr_registry_browse_filters.md` (D6 states the 500 window)
- `.agents/adr/adr_search_relevance_model.md` (assumed "a few thousand rows")
- `~/dev/index/.claude/artifacts/adr_enumeration_index.md` (OCX digest-map
  index; reserves a recursive pointer map for scale)
- Cap origin: commits bf267f54 (OCI walk), 56d72a92 (copied onto indexes)

## Open questions

- [NEEDS CLARIFICATION: Where is the index hosted, and is it fetched over HTTP or git?] Recommended: assume static HTTP hosting with unknown ETag/gzip behaviour — the two-level root is robust to both, and the git transport keeps the `all.json` path.
- [NEEDS CLARIFICATION: How is grim distributed to the fleet, and what is the oldest grim / extension version that must keep working?] Recommended: assume self-installed and unpinned, so `/all.json` must be served indefinitely — the same stance Cargo took.
- [NEEDS CLARIFICATION: What is the real publish/announce rate per day, and do ETags on the live host survive a no-op redeploy?] Recommended: measure both before choosing the Merkle ship date — they are the ship triggers.

## Verification

- Regression tests (red first): a 600-row index lists 600 rows with
  `truncated=false`; an `include` glob reaches a row past position 500; a
  queried search, `grim rate` and `grim status --check` resolve a row past 500.
- Compatibility matrix in acceptance tests: old grim × new index (only
  `/all.json` read), new grim × old index (404 probe → `/all.json`), new grim ×
  new index (Merkle sync); extension old/new against grim old/new.
- Merkle sync tests: 304 on an unchanged root; one release fetches exactly
  one shard and one record; a pruned record (404) or digest mismatch falls
  back to `/all.json`; cold start bootstraps from `/all.json`.
- Benchmark against a copy of a real index and a synthetic 20,000-entry corpus: warm
  search < 1 s and < 300 MB RSS; bytes per client per release < 100 KB.
- Indexer: a 20,000-entry build completes within the CI job limit.
- `task verify` in grimoire; indexer and extension test suites; JSON tripwire
  tests updated in the same commit as any additive field.
