# ADR: Index site at scale — 48 cards, one cached search file, and the P0 hardening patch

## Metadata

**Status:** Proposed
**Date:** 2026-10-21
**Deciders:** Michael Herwig (maintainer)
**Beads Issue:** N/A
**Related PRD:** N/A
**Blast radius:** one repo (`grimoire-indexer`: renderer, site, rendered CI); no grim change
**Reversibility:** two-way. Site pages, `c/q/` and blob placement are not contracts. The `browse/`
reservation is permanent; its owner is the design doc's IX-R02, which reserves it with `c/`.
**Tech Strategy Alignment:**
- [ ] Decision follows Golden Path in `.claude/rules/product-tech-strategy.md`
- [x] OR deviation justified: the indexer is a TypeScript and Astro repo outside grim's golden
      paths. New dependencies are `rehype-sanitize`, `rehype-raw` and `@shikijs/transformers`,
      pinned, plus `@astrojs/markdown-remark` at Astro's exact peer.
**Domain Tags:** security | frontend | devops
**Supersedes:** none. **Superseded By:** none.
**Moved from:** [`adr_catalog_at_scale.md`](./adr_catalog_at_scale.md) at the 2026-10-21 reset.
Mapping: S1 = former D24, S2 = former D25 (its page cap, budget and host ceiling), P0 = former D25's
first three bullets, M1 and M2 here = former M4 and M6. S3 to S5 are new.

## Context

The catalog ADR fixes how grim reads an index: `c/root.json`, content-addressed `c/` files, and the
frozen `/all.json`, `/stats.json` and `/p/<ns>/<name>/` paths. It does not decide what the index
*site* ships to a browser. Three problems are independent of that layout.

1. **Payload.** Today `index.html` serializes every package twice, as island props and as a
   `<noscript>` list. The browser then fetches `/all.json` on a keystroke. At 20,000 entries the page
   is tens of MB and the site holds three catalog copies.
2. **Host limits.** A full-site replace must fit 1 GB on both Pages hosts, and GitLab also caps
   file entries at 200,000. The renderer emits a full README page per package.
3. **A pre-existing security Block.** Third-party markdown renders unsanitized, no page carries a
   CSP, SVG logos are served from the index origin, and the validate workflows share a writable
   cache with the build. This does not depend on any layout, so it ships ahead of the layout work.

The reset also changed one premise: the site is a *new* client and needs no mutable path. The
stats blob is now a content-addressed `c/f/` file, so every file the site reads can be cached
without revalidation.

## Decision Drivers

- First paint and first interaction stay flat as the catalog grows (design point 20,000, ceiling
  100,000).
- The site fits both Pages hosts at the design point, with the host ceiling stated.
- No new infrastructure, credential or runtime dependency in the browser.
- The security fixes ship without waiting on the layout work.
- Principle 9: `/p/<ns>/<name>/` URLs, `/all.json`, `/stats.json` and the theme overlay stay frozen.

## Industry Context & Research

**Research artifacts:** [`research_static_site_search_at_scale.md`](./../research/research_static_site_search_at_scale.md),
[`research_static_host_ci_limits.md`](./../research/research_static_host_ci_limits.md),
[`research_catalog_scale_security.md`](./../research/research_catalog_scale_security.md).
**Key insight:** at 20,000 entries the search row is the card content, so there is no cheaper list to
defer. Whole-catalog sort, filter and counts need every row. One cached file beats per-row fetches,
and Range-based stores fail because neither host serves Range reliably.

## Considered Options

### M1 — Site search and browse

| Option | Weighted total (max 115) | Verdict |
|---|---|---|
| **A. 48 SSR cards + one content-addressed search file; Pagefind on trigger** | **99** | Chosen. Constant first paint; ~4.6 MB gzip once on first interaction; ceiling near 30,000–40,000 |
| B. Today: every card as island props, `/all.json` on keystroke | 71 | ~40 MB of props at 20,000; no ceiling |
| C. Pagefind now | 94 | Filter-only browse loads every chunk; re-reads every page at build |
| D. SQLite or Parquet over HTTP Range | 91 | Range is broken on both hosts |
| E. Search over the catalog plane | 82 | First keystroke fetches every record |

- **Version-bound premise.** C's two weak scores hold for Pagefind 1.5.2. On Pagefind main an empty
  term no longer requests chunks ([#1317](https://github.com/Pagefind/pagefind/pull/1317),
  unreleased) and `addCustomRecord` skips the page re-read. Re-scored, C is 106 against A's 99.
  C stays the candidate at trigger D; it also adds one fragment file per document and cannot sort
  globally by stats.

### M2 — Blob placement versus host limits

| Option | Weighted total (max 115) | Verdict |
|---|---|---|
| A. Blobs published under `c/b/`, full pages | 74 | ~3.7 GB at 20,000; content-type sniffing risk |
| **B. Blobs cache-only, pages capped at 24 KB, logos allow-listed** | **88** | Chosen. ~0.75 GB, ~110,000 GitLab entries |
| C. Blobs pushed to an OCI registry | 84 | Needs a registry push token |
| D. Object storage and CDN | 87 | The documented path past the host ceiling (S2) |

- It changes the earlier "published site as fallback" for blobs. Pages and blob placement are not
  contracts, so the change is reversible.

### M3 — Browser cache for the site's digest URLs

| Option | Verdict |
|---|---|
| **A. HTTP cache with `fetch(url, {cache: 'force-cache'})` on digest URLs** | Chosen. No state to manage; an entry is never re-downloaded while the browser keeps it |
| B. Plain `fetch` | A 10-minute `max-age` means an unchanged 4.6 MB file can download again after expiry |
| C. Service worker | Needs an offline goal; none exists. Adds update races and a CSP allowance |
| D. Cache API sweep | Only guarantees residency; needs coordinated cleanup across tabs first |
| E. IndexedDB of parsed rows | Saves one parse; costs schema versioning, quota handling and a sweep |

## Decision Outcome

**Chosen:** M1-A, M2-B, M3-A, and the P0 patch ahead of the layout work.

### S1 — Site search and browse (former D24)

- `index.html` carries 48 server-rendered cards, per-kind counts, the presence flags the island's
  chips and sorts read (`hasDeprecated`, `hasRatings`, `hasDownloads`, the deprecated count), `N`, and
  two digests: the search file's and the stats blob's. No island card props beyond the 48.
- **`index.html` is the site's root.** The site never reads `c/root.json`. Reading the pipeline root
  would add a no-cache round trip and couple the site to the pipeline. The root names the search file
  in its `search` member only so the search file is in the root's closure and retention keeps it.
- `c/q/sha256/<d>.json.gz` is the site-private search file, gzip-only and columnar so field names
  appear once: `{"schema": 1, "fields": [...], "rows": [[...]]}`. `<d>` is the SHA-256 of the
  **inflated** bytes, as for the `c/f` blobs (catalog ADR D2); the indexer's deterministic gzip
  writes it, with no raw copy and no sibling. `fields` is the union of the search fields
  (`lib/search.ts`) and the card fields (`CARD_FIELDS`, `lib/catalog.ts`) minus `rating` and
  `downloads`: `ref`, `kind`, `name`, `title`, `keywords`, `tags`, `summary`, `description`,
  `namespace`, `vendor`, `authors`, `license`, `compatibility`, `version`, `created`, `updated`,
  `deprecated`, `replacedBy`, `logo`. Every row carries every field, `null` when absent. No stats, no
  `generation`: a stats-only build leaves its digest unchanged. Cap 32 MiB. The decoder rejects
  another `schema`, a `fields` list missing any of the 19 and a row of the wrong width; a rejection
  takes the S3 miss ladder.
- It loads on the first interaction: a keystroke, a kind or keyword chip, a sort change, the
  deprecated toggle, or a scroll past the 48. Until then counts read "48 of N". A restored non-default
  URL or stored view loads it on mount, with a pending state and never a sort over a partial set.
  `kw` validates against the loaded rows, so `replaceState` never erases a deep link.
- The 48 are the first 48 non-deprecated rows under the island's exported `compare(a, b, 'name')`;
  `index.astro` imports it, so there is no second comparator.
- **Stats** load lazily, only for badges and for a rating or downloads sort, from the gzip `c/f/`
  blob that `index.html` names, decoded as in S3. `/stats.json` stays frozen and unread by the site.
- **Theme overlay.** The 48-card page applies only while the shipped `Catalog.tsx` and
  `pages/index.astro` are in use. A theme that replaces either gets every card inlined, as today,
  with a build warning through the overlay's replaced-path lines. `data.packages` stays whole,
  because every `/p/` page builds from it.
- **Static browse.** `browse/<n>/` (200 entries per page) is the `<noscript>` target and the crawler
  path. Every `/p/<ns>/<name>/` is linked exactly once. The sitemap lists `browse/` and no `c/` path.
  This deliberately starts the paged listing before the size trigger, because the 48-card page drops
  the full `<noscript>` list. `browse/` is reserved like `c/` (IX-R02).
- **Gzip-only, so no sibling.** The search file is gzip on both hosts: wire bytes equal file bytes,
  GitLab needs no `.gz` sibling, and a retained file is copied as written. This removes the
  regenerated-sibling step (catalog design §5.8).
- **Bound:** `size(index.html at 20,000) − size(index.html at 100)` ≤ 4 KB.
- The marketplace page keeps its record-array path (`searchUrl="/marketplace/all.json"`).
- **Trigger D** (S2) switches search to Pagefind and browse to per-kind listings. A follow-up ADR
  owns it. The 32 MiB search-file cap is reached near ~33,500 entries, inside D's band.

### S2 — Site budget, page cap and host ceiling (former D25)

- **Per-package pages are capped at 24 KB of emitted HTML**, measured after the CSP post-process,
  because Shiki output expands several times. Staging works on a copy, never through the `enrich/`
  symlink. It cuts the changelog first, then the contents, then the README, and links to the
  repository and `grim fetch <ref> --description`. At most two renders, then a hard cut with a
  "truncated" notice, inside a 5-minute staging budget; past it every remaining page takes the hard cut.
- **Structured sections** bypass the markdown cuts, so each has a bounded preview and a link-out
  form: 32 bundle members, the MCP descriptor's JSON cut within 2 KiB, 32 older tags, 32 download
  rows, the summary line at 1 KiB. With every section in link-out form the page has a fixed floor,
  asserted ≤ 24 KB, so the cap holds for every page the shipped template renders. Only a theme that
  replaces `PackageDetail.astro` can exceed it, and it deploys with a warning.
- **Budget at 20,000** (estimate, unmeasured, rounded): ~38 KB per entry, ~0.75 GB tar, ~110,000
  GitLab entries. That base holds the raw legacy pair (~44 MB). The gzip-only `c/f/` pair is ~8 MB.
  - With `legacy: true` the site adds the `c/f/` pair, the two `.gz` siblings (~8 MB) and the
    retained previous closure (~13 MB: the `c/f/` pair and the search file; unchanged enrich shards
    are shared): +0.03 GB, ~0.78 GB in all. The build after an enricher bump or a full rotation also
    retains every old enrich shard (~28 MB): ~0.81 GB.
  - With `legacy: false` the gzip pair replaces the raw legacy pair (−0.04 GB) and the previous
    closure adds ~0.01 GB: ~0.72 GB.
  - No host-headroom cap: one previous closure is at most ~6% of the tar.

| Host limit | Ceiling |
|---|---|
| Trigger E, 80% of the 1 GB site | ~20,000 entries (`legacy: true`), ~22,000 (`legacy: false`): the practical ceiling, at the design point |
| 1 GB site (both hosts) | ~25,000 (`legacy: true`) |
| 200,000 GitLab entries | ~36,000 |
| Search file under the 8 MB wire trigger | ~30,000–40,000 |

**Triggers.** Both are read from the build log line, which reports the search file's bytes as served
(`search wire`), `site bytes` and `entries`. They are
log text, not a contract.

| Trigger | Fires when | Action |
|---|---|---|
| D | Search-file wire bytes ≥ 8 MB (warns at 6 MB) | Pagefind and per-kind browse listings (follow-up ADR) |
| E | Site bytes ≥ 80% of 1 GB, or entries ≥ 80% of 200,000 | Object storage and a CDN (below) |

- **Astro at scale:** `experimental.collectionStorage: 'chunked'`, so Astro is pinned exactly.
  Without it the single content store passes V8's string limit near 37,000 entries. `deferRender`
  stays off: measured at 20,000 entries it makes each markdown file its own bundler module
  (15.3 GiB and 843 s, against 5.8 GiB and 262 s without it). Synthetic-build budget at 20,000:
  the Astro step ≤ 15 min, `__GRIMOIRE_DATA__` included. The build makes no claim on memory:
  peak RSS and the `.astro/` size are measured and reported, not judged, until someone hits a
  limit (indexes hold about 500 entries today).
- **Past the ceiling**, and for 100,000, the site moves to object storage and a CDN. Paths stay
  relative (`withBase`), so this is a deploy-target change. A non-atomic store needs serialized
  publication, one deployer at a time: upload and verify every content-addressed file, then switch
  the two mutable roots, `c/root.json` and `index.html`, then prune files outside the current and
  previous closures. A root must never name a file the store lacks. Profile:
  `c/root.json`, `all.json` and `stats.json` get a short `max-age` or `no-cache`; the
  content-addressed `c/{p,s,f,q}/sha256/` files a long one; a custom domain, never `r2.dev`; a 403
  for a missing key reads as a miss. Cloudflare Pages and Workers static assets cap a site at 20,000
  files, below the ~110,000 needed.
- A post-deploy probe checks `Content-Type` on one root, record, logo and page (warn-only).

### S3 — Browser cache (new)

- Every `c/` fetch from the page uses `fetch(url, {cache: 'force-cache'})`. The URL changes whenever
  the bytes change, so an unchanged digest is never downloaded or revalidated while the browser keeps
  it. This corrects the earlier claim that content addressing "buys no caching".
- **Decode.** Every `.json.gz` the page reads is checked for the gzip magic. Bytes starting `1f 8b`
  go through `DecompressionStream('gzip')`, aborting past the file's size cap; anything else was
  decoded in transit by a host or CDN and is parsed as it is.
- No service worker, no Cache API, no IndexedDB. The reasons are: no offline goal, no measured
  eviction problem, and the HTTP cache is the one store tracking-prevention rules leave alone. Add a
  Cache API sweep only if a field measurement shows eviction causing repeat 4.6 MB downloads, and
  only after normalizing URLs and coordinating cleanup across tabs.
- **Cleanup is automatic.** The browser evicts superseded digests as LRU entries. The server prunes
  `c/` under the catalog ADR's retention. No client sweep exists.
- **Retention dependency.** A cached `index.html` can be up to one host `max-age` old and names a
  search digest and a stats digest. The catalog ADR keeps the previous root's closure, which holds
  both, so one build of lag is covered. Retention is best effort, so recovery is mandatory.
- **Miss ladder** on a failed digest fetch, every URL through `withBase`:
  1. Retry the same URL once with `cache: 'reload'`. A cached 404 must not stick, because
     `force-cache` also returns a stored failure.
  2. Reload `index.html` with `cache: 'reload'`, read the fresh digest, fetch it.
  3. Last resort: substring search over the server-rendered cards, with a notice and a link to the
     `browse/` pages.
  Never `/all.json`: it is absent when `legacy` is false. A stall fails a load, not a total timeout
  (`lib/search.ts:50` has 15 s total today), so a slow link still finishes.
- **Unverified premise**, not stated as fact: that the hosts send `max-age=600` and re-stamp
  validators on every deploy (one low-authority source). The design is correct either way, because
  `force-cache` does not depend on validators and the gzip-only files do not depend on host
  compression.

### S4 — Deferred: streaming, virtualization, lazy rows (new)

- **Ship the whole-file path first:** `fetch` plus `response.json()` over the columnar file, keeping
  the 48 SSR cards. NDJSON streaming is deferred until a throttled 20,000-entry phone trace shows
  first interaction is the bottleneck. A keystroke needs every row anyway.
- **If streaming is adopted**, these island changes are required, each with a regression test:
  - key the render-window reset on filter and sort inputs, not on `shown` identity
    (`Catalog.tsx:1256-1261` resets to 48 on every batch);
  - coalesce appends on a time budget, skip the sort while streaming in name order, and yield with
    `scheduler.yield()` or a message-channel tick, since `requestAnimationFrame` stalls in a hidden tab;
  - one locale-independent comparator (code-point order on name, then ref) in the build, the island
    and the SSR cut, with a parity test across `en`, `sv` and `tr`;
  - define the SSR 48 as the first 48 non-deprecated rows, and gate progressive results on ascending
    name order only;
  - show "k of N loaded" outside the `role="status"` live region, set `aria-busy`, and announce once
    on completion;
  - validate header, field layout, row widths, declared count and duplicates, so a truncated stream
    is never certified by a clean EOF; cancel the request before recovery.
- **No virtualizer** until a 20,000-entry measurement. The trigger is a phone-survivable DOM budget or
  a scroll-to-end long-task budget, not 1.5 M nodes. Then use `@tanstack/react-virtual` through
  `preact/compat`, table view only.
- **Fetching data while scrolling over hash-bucketed shards is rejected.** Buckets are in hash order,
  so sort, filter, counts and search are wrong until every bucket loads, and one screen costs ~96
  requests. Per-row lazy fetch from a compact key list is a follow-up ADR for the 100,000-entry CDN site.

### S5 — What breaks first (new)

- **The landing page** embeds every package twice. Fix: S1's 48 cards, one listing file and static
  `browse/<n>/` pages, which replace both full-catalog payloads. Without the browse pages the
  `<noscript>` list is gone and no-JS users are stranded after 48 entries.
- **`sitemap.xml`** allows 50,000 URLs per file. Emit a sitemap index before 100,000.

### P0 — Indexer hardening package

Ships as its **own indexer PR**, independent of both ADRs and of every layout. It is released from
indexer main, not backported to the 0.5.x line. Main needs grim ≥ 0.15.0, so P0 ships after the grim
0.15.0 release.

- **Sanitizer.** Astro 7's default Sätteri processor takes no rehype plugins, so set
  `markdown.processor: unified({...})` from `@astrojs/markdown-remark`, pinned to Astro's exact peer.
  The verified order is Shiki, then the user plugins `rehype-raw`, `rehype-sanitize` (pinned schema)
  and a fragment-link fix, then Astro's image and heading-id steps, so no raw HTML bypasses the
  sanitizer. `clobberPrefix: ''` stops double-prefixed footnote ids. The schema admits classes by
  pattern only (`astro-code`, the two theme names, `^__shiki_`). A spike confirms the chain and diffs
  a README corpus under Sätteri and `unified()` first, since the switch changes every page's markup.
- **Shiki classes.** `transformerStyleToClass({classPrefix: '__shiki_'})` replaces inline colours with
  one stylesheet, so the CSP needs no `'unsafe-inline'`. `CodeBlock.astro` passes the same
  transformer from one shared options object, because Astro's `<Code>` does not inherit
  `shikiConfig`. The post-process fails the build on a `__shiki_` class without a rule.
- **CSP.** A post-process over every emitted `dist/**/*.html` injects a hash-based
  `<meta http-equiv>` as the first child of `<head>`, so a theme layout cannot drop it. Directives:
  `default-src 'self'`; `script-src` and `style-src` `'self'` plus the hashes of allow-listed inline
  blocks (including the `<noscript>` style); `img-src 'self' https:`; `connect-src 'self'`;
  `object-src 'none'`; `base-uri 'self'`; `form-action 'self'`. No `unsafe-inline`, `unsafe-eval` or
  `unsafe-hashes`. An inline script, style or `style=` inside the rendered-markdown subtree fails the
  build. The static hosts set no headers, and Astro's `security.csp` does not cover Shiki.
- **Logos by magic bytes**, extension derived from the bytes: package logos PNG, JPEG, WebP or GIF;
  marketplace logos PNG or WebP. One choke point covers publish, checkpoint or seed restore and the
  marketplace reader. SVG, extensionless and mismatched files are dropped and `logo` becomes `null`.
  An SVG opened top-level on the index origin runs script.
- **Cache-writer scoping.** The `pull_request_target` validate job and verify-ci drop `cache: npm`.
  Every workflow and job but the default-branch build job sets `cache-mode: none` (rendered only on
  `github.com`). GitLab merge-request jobs set `cache: []`; GitLab jobs get `timeout: 2h`.
- **Reach.** `npm ci` pins the lock, so P0 reaches an index only on a lock bump plus a re-render. The
  release notes and the `ci:check` hint say so. A fork-PR check of the `pull_request_target`
  checkout is a release-checklist step, since Vitest cannot run it.

### Consequences

**Positive:** flat first paint (≤ 4 KB growth); repeat visits cost nothing; the security Block closes
without waiting on layout work; the site never couples to the pipeline root.
**Negative:** truncated pages link out; the first keystroke waits on one ~4.6 MB gzip file
(estimate, unmeasured); a browser decode step (`DecompressionStream` with the magic-byte check) is new code.
**Risks:** HTTP-cache eviction on mobile is unmeasured; a stale `index.html` can 404 on a dropped
digest, which the miss ladder absorbs at the cost of one extra fetch.

## Requirements

All land in `grimoire-indexer`. IDs are the former design-doc rows, kept for traceability and not
reused. Phase P0 ships first among the indexer work, after grim 0.15.0.

| Former ID | Requirement | Phase |
|---|---|---|
| IX-R14, R37, R38 | 48-card `index.html`, flags, lazy search file and stats, mount rules (S1) | P2b |
| IX-R15 | Search file in every layout, gzip-only `.json.gz` with the magic-byte decode; stall-based load failure; S3 miss ladder replaces the `/all.json` rung | P2b |
| IX-R16 | `browse/<n>/`, one link per `/p/` page, sitemap without `c/`; sitemap index before 100,000 | P2b |
| IX-R17, R46 | 24 KB page cap, staging bounds, structured-section previews, floor fixture (S2) | P2b |
| IX-R18, R19, R20, R44 | Sanitizer, Shiki classes, CSP, logo choke point, cache scoping, release notes (P0) | P0 |
| IX-R24, R32, R33 | Post-deploy `Content-Type` probe; gzip-only `c/q` with no sibling (deterministic gzip: catalog design IX-R52); Astro build settings and budgets | P2b |
| IX-R39 | Theme-overlay carve-out for the 48-card page | P2b |
| new | `force-cache` on every digest URL; one `cache: 'reload'` retry; `index.html` reload rung; all URLs via `withBase` | P2b |

## Validation

- **Site:** `index.html` growth ≤ 4 KB from 100 to 20,000 entries; one search-file load; each ladder
  rung, including a stalled load and a cached 404 that recovers on retry; the paged listing links
  every page once; every `dist/p/**/index.html` ≤ 24 KB, including the structured fixtures (5,000
  bundle members, an MCP descriptor at the 1 MiB ingest cap, 128 tags with a 16 KiB description)
  and the worst-case floor; a zero staging budget still deploys. The search file round-trips the
  full 19-field union, each field set and each absent, and `loadSearchIndex` still accepts the
  marketplace record array; a retained `q` file is copied byte-identical after a full-site replace; the page decodes a gzip body and a host-decoded one.
- **Object-store publication:** an interleaved upload, delete and deploy test never leaves
  `c/root.json` or `index.html` naming a missing file, and two concurrent deployers cannot prune each
  other's closure.
- **P0:** the CSP is the first child of `<head>` with exactly the directive list; Shiki output carries
  classes only, from the markdown processor and `CodeBlock` alike; hostile and fidelity sanitizer
  corpora; logo fixtures including a PNG+HTML polyglot served as `image/png`; browser smoke with zero
  CSP violations on `/`, a `/p/` page, an MCP detail page, a theme page and `/marketplace/`, with
  JavaScript off too; a build with the sanitizer disabled trips `script-src`.
- **Island at 20,000 rows:** each chip, sort and toggle before and after the load; counts read
  "48 of N" until the load; stats load only on a rating or downloads sort; every restored-state
  case; the first 48 unchanged; `loadSearchIndex` still accepts a record array; a replaced
  `Catalog.tsx` or `index.astro` inlines every card and warns.
- **Interaction trace (acceptance, not a maybe):** extend the bulk Lighthouse task with a scripted
  scroll-to-end, sort and search run, throttled to mobile CPU, with long-task and DOM budgets. The
  current run audits `/` at load only and misses every interaction cost.
- [ ] Security review of the P0 patch before release.
- [ ] Measure `curl -I` headers and `.json` compression on both hosts before quoting either.

## Open Questions

Resolved by the owner on 2026-10-22:

1. P0 cut point: released from indexer main, not backported; it needs grim ≥ 0.15.0, so it ships
   after the grim 0.15.0 release (P0).
2. Reserving `browse/` and the theme-overlay tier: amend the tier with a CHANGELOG notice; exit 65
   only on an actual collision (IX-R02; catalog ADR Open question 18).

## Links

- [`adr_catalog_at_scale.md`](./adr_catalog_at_scale.md) — `c/` layout, `c/f/` blobs, `legacy`,
  retention
- [`design_catalog_at_scale.md`](../specs/design_catalog_at_scale.md) — grim and indexer requirements
- [`adr_artifact_trust_model.md`](./adr_artifact_trust_model.md) — the boundary P0 hardens
- [`research_static_site_search_at_scale.md`](../research/research_static_site_search_at_scale.md)

---

## Changelog

| Date | Author | Change |
|------|--------|--------|
| 2026-10-21 | architect | Initial draft: site decisions, P0 package and the browser-cache decision, moved out of `adr_catalog_at_scale.md` at the reset |
| 2026-10-22 | architect | owner answers to the open questions folded in (2026-10-22) |
| 2026-10-22 | architect | KISS cut (owner): retention is the previous root's closure, which names the search file via the root's `search` member; no host-headroom cap; `c/r` dropped with the deferred tree |
| 2026-10-23 | owner | IX-R33 amended: the peak-RSS (4 GiB) and `.astro/` (2 GiB) build budgets dropped, measured and reported only; `deferRender` removed from the Astro settings (measured at 20,000 entries: 15.3 GiB and 843 s against 5.8 GiB and 262 s), `collectionStorage: 'chunked'` kept |
