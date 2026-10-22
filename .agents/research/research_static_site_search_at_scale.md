# Research: Static-Site Search and Browse at Catalog Scale

## Metadata

- **Date:** 2026-10-20
- **Lane:** prior-art, opt-in (user-requested)
- **Question:** how do sites serve search plus browse/filter over a large catalog (10k–100k items, ~1 KB searchable text each: name, title, summary, description, keywords, tags, kind, namespace, license) from static hosting only (GitHub Pages, GitLab Pages, S3/CDN), with no server-side logic?
- **Output rule:** findings, `negative:`, `leads:` only. No recommendation.
- **Evidence tags:** `[cited]` = read from the linked source. `[measured]` = measured by this run on 2026-10-20 (method in "Measurement method"); the corpus is synthetic, so treat as order-of-magnitude. `[unverified]` = reported by a secondary source only.

## Summary table

Sizes are over the wire (gzip as served) unless marked raw. "100k" rows for the whole-index loaders and Pagefind are `[measured]` on the synthetic corpus below; other rows are `[cited]`.

| Approach | Index chunking | First-query bytes | Index vs source | Browser memory | Prefix / fuzzy | Facets, no-query browse | Known failure at scale |
|---|---|---|---|---|---|---|---|
| Pagefind | sorted-word ranges, ~20000 location-units per chunk, content-hashed names; one fragment file per document; one filter file per filter key | 100k docs: ~1.0 MB (meta 882 KB + wasm 71 KB + one chunk 40–110 KB) + 45 KB js | 100k: 106 MB output vs 146 MB source (72%) | not measured (wasm + loaded chunks + per-page metadata) | prefix yes; no edit-distance found | `filters()` counts and `search(null, {filters})` supported; **empty-term browse loads every index chunk (39.7 MB at 100k)** | 100k files in `fragment/`; common short terms freeze the UI; crashes at 300k–500k pages (forum report) |
| Stork | one index file | whole file | n/a | whole index | prefix yes | none documented | project wound down (2023) |
| tinysearch | everything inside one wasm | whole wasm | ~2 KB raw per document (~1 KB compressed) per README | whole index | prefix from 3 chars; fuzzy no | none | README: "small- to medium-size websites" |
| MiniSearch | none (one JSON) | 100k: 29.6 MB gz / 17.4 MB br | 100k: 102.6 MB raw vs 146 MB source | 100k: ~570 MB heap (Node) | prefix + fuzzy yes | `filter` callback only, no facet counts | design doc lists large index size as a non-goal |
| lunr | none (one JSON) | 100k: 49.1 MB gz / 31.3 MB br | 100k: 158 MB raw | 100k: 1.16 GB heap after load, 2.2 GB after build (Node) | wildcard + edit-distance yes | none | maintainer: not meant to compete with Solr/Elasticsearch |
| FlexSearch | none (export emits keyed parts, import loads all) | 100k: 86.3 MB gz / 32.6 MB br | 100k: 374 MB raw (`forward` tokenizer) | 100k: 1.07 GB heap (Node) | partial + phonetic; tag filter | tag search | persistent indexes need IndexedDB/DB, not static files |
| Orama | none (one JSON/binary export) | 100k: JSON export not producible (V8 string cap) | ~4x source as raw JSON (smaller corpus) | not measured at 100k | typo tolerance yes | facets built in | JSON export over Node's string limit at 100k |
| rustdoc / stringdex | partitioned suffix tree, merkle-hashed node files bundled by size; descriptions in 128 KiB shards | not published | std 16 MiB, compiler 49 MiB on disk | not published | prefix, substring, Levenshtein | n/a (name + type search) | adopted Aug 2025; disk size grew vs flat list |
| SQLite over Range (sql.js-httpvfs) | byte ranges over B-tree pages; optional 10 MiB file chunks | schema pages + ~70 KiB per FTS query (8 MB FTS table) | 100k: 289 MB db vs 146 MB source | page cache, no eviction | FTS5 prefix (`prefix='2 3'`), trigram substring; no fuzzy | plain indexed SQL: filter + ORDER BY + LIMIT | GitHub Pages gzip breaks Range; GitLab Pages ignores Range |
| DuckDB-WASM + Parquet | byte ranges: footer, then column chunks per row group | footer + needed columns | n/a | WASM heap | `LIKE`/`ILIKE` scan, no FTS index | SQL filter, row-group pruning | falls back to full download when Range is not honoured |
| MDN autocomplete | one title+URL JSON per locale | 144 KB (brotli) | titles only: 14,706 entries, 1.4 MB raw | all in memory | FlexSearch over titles | none | full-text search is a separate backend |
| Homebrew formulae | none | n/a | `formula.json` 32.2 MB raw | n/a | search is Algolia DocSearch (hosted) | Algolia facet filter | needs a hosted service |

## 1. Pagefind

**Chunking policy** `[cited]`
- Words are sorted, then packed into chunks until a running count of word locations reaches the chunk size; default `20000` ([`options.rs` `default_index_chunk_size`](https://github.com/Pagefind/pagefind/blob/main/pagefind/src/options.rs), [`index/mod.rs` `chunk_index`](https://github.com/Pagefind/pagefind/blob/main/pagefind/src/index/mod.rs)). The unit is locations, not bytes; a source comment says "TODO: Parameterize these chunk sizes via byte size rather than word count". The setting is only exposed as the unstable env var `PAGEFIND_UNSTABLE_INDEX_CHUNK_SIZE`.
- Each chunk records its first and last word in the meta file (`MetaChunk { from, to }`), so the client picks chunks by word range. A prefix query that falls on a chunk boundary was a historical bug ([CHANGELOG](https://github.com/Pagefind/pagefind/blob/main/CHANGELOG.md): searching `h` with a first chunk starting at `hello` returned nothing).
- Layout: `index/<hash>.pf_index` (word chunks), `fragment/<hash>.pf_fragment` (one per document), `filter/<hash>.pf_filter` (one per filter key), `pagefind.<lang>_<hash>.pf_meta`, `wasm.<lang>.pagefind`, `pagefind-entry.json` (the only unhashed pointer) ([`output/mod.rs`](https://github.com/Pagefind/pagefind/blob/main/pagefind/src/output/mod.rs), [`coupled_search.ts`](https://github.com/Pagefind/pagefind/blob/main/pagefind_web_js/lib/coupled_search.ts)).
- Files are gzipped by Pagefind itself and gunzipped in JS (`decompress` in `coupled_search.ts`), so serving does not depend on host `Content-Encoding`.
- Vendor claim: 10,000 pages under ~300 kB total network, usually ~100 kB ([pagefind.app](https://pagefind.app)); that is for prose pages with no filters, not verified by an independent benchmark.
- Index chunks after v1.x are ~45% smaller from delta-encoded page numbers; chunk fetches are throttled for large sites; search runs in a Web Worker ([CHANGELOG](https://github.com/Pagefind/pagefind/blob/main/CHANGELOG.md)).

**Facets and browse** `[cited]`
- Filters are declared with `data-pagefind-filter`; a null term runs a filter-only search ([filtering](https://pagefind.app/docs/filtering/), [JS API filtering](https://github.com/Pagefind/pagefind/blob/main/docs/content/docs/js-api-filtering.md)); `pagefind.filters()` returns per-value counts. Sorting by a tagged attribute is supported ([sorting](https://github.com/Pagefind/pagefind/blob/main/docs/content/docs/js-api-sorting.md)); the build stores one pre-sorted page-number array per extra sort key (`alternate_sorts` in [`index/mod.rs`](https://github.com/Pagefind/pagefind/blob/main/pagefind/src/index/mod.rs)).
- Multiple bundles can be merged in the browser (`mergeIndex`) ([multisite](https://github.com/Pagefind/pagefind/blob/main/docs/content/docs/multisite.md)).

**Known failures at scale** `[cited]`
- Short common terms freeze the page on a 29,977-page / 99,142-word / 225-chunk corpus ([Pagefind#49](https://github.com/Pagefind/pagefind/issues/49)); a 180k-page site reported the same.
- File count: one fragment per page; Cloudflare Pages caps at 20k files ([Pagefind#739](https://github.com/Pagefind/pagefind/issues/739)); a `--group-fragments N` option was proposed ([Pagefind#74](https://github.com/Pagefind/pagefind/issues/74)). `negative:` I did not find that it shipped.
- A Hugo forum thread reports crashes at ~300k pages, "too hard to scale with even a site of 500k", and 250k as the largest working build ([Hugo forum](https://discourse.gohugo.io/t/scaling-pagefind-to-over-1-million-pages/56109)); the proposed workaround there (per-prefix sub-sites) was a suggestion, not a tested result.

**Measured here** `[measured]`, Pagefind 1.5.2 release binary (sha256 verified), synthetic corpus below:

| | 100,000 docs |
|---|---|
| source HTML | 146.4 MB |
| `index/` | 999 files, 40.6 MB (median 28 KB, p95 97 KB, max 616 KB) |
| `fragment/` | 100,000 files, 63.7 MB |
| `filter/` | 3 files, 777 KB (~260 KB each, even for 6-value `kind`) |
| `.pf_meta` | 904 KB gz (1.79 MB raw) |
| wasm + js | 72 KB + 46 KB |
| indexing time | 42 s |
| bytes, cold word query, top 10 results | 1,068 KB (chunk 109 KB, 10 fragments 6 KB) |
| bytes, cold prefix `conf` | 1,014 KB |
| bytes, term + `kind` + `namespace` filters | 1,573 KB (2 filter files 505 KB) |
| bytes, `filters()` counts only | 1,712 KB |
| bytes, **no term, `kind=agent`, first 20 results** | **40.9 MB (all 999 chunks)** |
| bytes, no term, sort by name | 40.5 MB |
| typing `s`→`sk`→`ski`→`skil`→`skill`→`skills` (warm) | 997 KB, +115 KB, +1.6 KB, 0, 0, +33 KB |

- The meta file grows linearly with documents (~9 B/doc gz): every first query pays it.
- Cause of the browse cost (`coupled_search.ts`, `search()`): `request_indexes(ptr, term)` is called with the empty string for a null term and returns every chunk. `negative:` I found no documented way to browse by filter without loading the whole word index.
- Node-side CPU for a term matching 56k of 100k documents: 6.7–7.9 s per keystroke on the main thread (indicative; the browser uses a Web Worker).
- `negative:` the "typo" probe `skil` is a prefix of `skill`, so it does not test edit distance; I found no fuzzy or typo option in `docs/content/docs`.

**Hourly rebuild** `[measured]`
- Identical input rebuilt: `index/` and `fragment/` names identical (0 of 247 index chunks and 0 fragments renamed); the 3 `filter/` files were renamed on every identical rebuild (non-deterministic filter hash in 1.5.2; cause not investigated).
- One document edited with existing vocabulary: 3 of 247 index chunks, 1 fragment, the meta file and the 3 filter files got new names.
- 15 documents edited, each adding one new vocabulary word: all 247 index chunks got new names (chunk boundaries are cumulative, so an insertion shifts every later chunk).
- Reuse of already-written hashed files is documented in the changelog ([CHANGELOG](https://github.com/Pagefind/pagefind/blob/main/CHANGELOG.md): "existing hashed files will be reused if present"). The tool always re-reads all source HTML; there is no incremental indexing mode that I found.
- `leads:` the unhashed `pagefind-entry.json` is the only entry point, so a stale entry file with purged hashed files is the failure to watch on S3/CDN (old hashed files must stay until caches expire).

## 2. Whole-index loaders

`[cited]` Stork: one `.st` index file registered by URL ([README](https://github.com/jameslittle230/stork/blob/master/readme.md)); the author stopped work in 2023 and points to tinysearch and Pagefind ([discussion 360](https://github.com/jameslittle230/stork/discussions/360)).

`[cited]` tinysearch: all indexes are bundled into one wasm; "Expect around 2 kB uncompressed per article (~1 kb compressed)"; default exact indexer does prefix matching from 3 characters; the optional `xor8` bloom-filter indexer supports prefixes in titles only; recommended only for "small- to medium-size websites" ([README](https://github.com/tinysearch/tinysearch)). Extrapolating the README figure to 100k items gives ~200 MB raw / ~100 MB compressed `[derived, not measured]`. The design derives from per-document bloom filters ([Stavros](https://www.stavros.io/posts/bloom-filter-search-engine/)), which scan every document filter per query.

`[cited]` lunr: prebuild and ship the serialised JSON; "the time taken to build the index can lead a browser to block" ([guide](https://lunrjs.com/guides/index_prebuilding.html)); wildcards, boosts and edit-distance matches ([searching guide](https://lunrjs.com/guides/searching.html)); leading wildcards "can have a negative impact on the performance of a search, especially in large indexes". An 800,000-entry in-browser build took over 15 s ([lunr issue mirror](https://lightrun.com/answers/olivernn-lunr-js-lunr-with-a-large-index-800000-items)) `[unverified, mirror]`.

`[cited]` MiniSearch: goals list "small memory footprint"; non-goal "a solution for use cases requiring large index data structure size" ([DESIGN_DOCUMENT](https://github.com/lucaong/minisearch/blob/master/DESIGN_DOCUMENT.md)); prefix, fuzzy, boosting ([README](https://github.com/lucaong/minisearch)).

`[cited]` FlexSearch: `export()` writes several keyed parts, `import()` loads them into memory; persistent and worker indexes do not support export/import ([export-import](https://github.com/nextapps-de/flexsearch/blob/master/doc/export-import.md)); persistence targets are IndexedDB and databases ([README](https://github.com/nextapps-de/flexsearch)).

`[cited]` Orama: whole-instance `save`/`load` through `@orama/plugin-data-persistence` with `json`, `dpack`, `binary`, `seqproto` formats ([source](https://github.com/oramasearch/orama/blob/main/packages/plugin-data-persistence/src/index.ts)); facets and typo tolerance in the engine ([README](https://github.com/oramasearch/orama)). `negative:` the official docs site is client-rendered and returned no body to my fetcher; I found no official statement of a document-count ceiling.

**Measured** `[measured]` (Node 24, same synthetic corpus, indexed fields name/title/summary, so ~1.4 KB per document; default tokenisers except FlexSearch `forward`; libraries store different amounts, so sizes are not like-for-like):

| Library (version) | Docs | Serialised raw | gzip -9 | brotli | Load (parse + restore) | Heap after load | Build |
|---|---|---|---|---|---|---|---|
| MiniSearch 7.2.0 | 100k | 102.6 MB | 29.6 MB | 17.4 MB | 1.8 s | 571 MB | 18.7 s |
| lunr 2.3.9 | 100k | 158.2 MB | 49.1 MB | 31.3 MB | 1.5 s | 1.16 GB | 33 s |
| FlexSearch 0.8.212 | 100k | 374.1 MB | 86.3 MB | 32.6 MB | 4.4 s | 1.07 GB | 34 s |
| Orama 3.1.18 | 100k | not produced: `JSON.stringify(save(idx))` threw `RangeError: Invalid string length` | n/a | n/a | n/a | n/a | n/a |

- Orama at 100k: on a smaller corpus of the same shape the JSON export ran ~4x its source HTML, so at 100k (146.4 MB source) it would be ~590 MB, above Node's `MAX_STRING_LENGTH` of 536,870,888 characters; the JSON persistence path fails before any file exists. The `binary`, `dpack` and `seqproto` formats were not tried.
- Reading: at 100k the smallest transfer is 17.4 MB brotli (MiniSearch), and the heap is 0.57–1.16 GB for the three that produced a 100k export (MiniSearch 571 MB, FlexSearch 1.07 GB, lunr 1.16 GB). All of it is paid before the first query. `negative:` none of the four exposes a lazy or partial load.
- Rebuild/invalidation: the whole file changes on any edit; no incremental form. Docusaurus' local-search plugin hashes the index filename or query and can split into multiple indexes per path context ([README](https://github.com/easyops-cn/docusaurus-search-local), `hashed`, `searchContextByPaths`); Orama's Docusaurus/Astro/Nextra/VitePress plugins live in the same monorepo ([packages](https://github.com/oramasearch/orama/tree/main/packages)).

## 3. rustdoc: description shards, then stringdex

`[cited]` Two successive changes, both in rust-lang/rust:

1. **Description sharding**, [#122614](https://github.com/rust-lang/rust/pull/122614) (merged via rollup [#123372](https://github.com/rust-lang/rust/pull/123372), 2024-04-02, Rust 1.79). Descriptions "make up most of the search index on almost all crates" but are not searched, so they move to separate shard files; an empty-description bitmap (cost ~16 KiB on std) lets results be sorted and truncated before any description is fetched; only the visible results' shards are downloaded, in parallel. std sections: names 800K, function signatures 544K, implementations 192K, types 64K, parents 36K, aliases 20K, bitmap 16K, about 64 thousand items (author's estimate). A later change cut the shard size from 1 MiB to 128 KiB; the current rustdoc says there is "no single, optimal size" because it depends on HTTP version, transport compression and network ([`DESC_INDEX_SHARD_LEN`](https://doc.rust-lang.org/stable/nightly-rustc/rustdoc/html/render/search_index/constant.DESC_INDEX_SHARD_LEN.html)). Author's own benchmark on stm32f4: wall time 6.9 s to 3.9 s, peak RSS 584 MB to 539 MB, flagged as "janky and unscientific".
2. **Name search moved to a partitioned suffix tree**, [#144476](https://github.com/rust-lang/rust/pull/144476) (merged 2025-08-19): a new crate, `stringdex`, gives prefix, substring and modified-Levenshtein matching and is "incremental" (exact matches first, nothing re-ordered under the user's cursor). Design ([HACKING.md](https://gitlab.com/notriddle/stringdex/-/blob/main/HACKING.md)): Ukkonen suffix tree; a `suffixes_only` flag so prefix and word matching never load suffix-only subtrees; nodes serialised into files named by their subtree hash (a Merkle tree, "HTTP cache busting for free" plus de-duplication); several nodes bundled per file based on size; roaring bitmaps for matches; missing dependencies lazy-loaded. Stated drawback: "takes up more disk space than the current flat list of strings".
   - Numbers: std docs `search.index/` 18M to 16M and compiler docs 57M to 49M on disk after [#147002](https://github.com/rust-lang/rust/pull/147002); further format tweaks in [#145911](https://github.com/rust-lang/rust/pull/145911) ("shouldn't significantly increase the amount of downloaded index data") and [#147660](https://github.com/rust-lang/rust/pull/147660). `negative:` I found no published bytes-per-query figure for stringdex.
   - Scope: item names and types, not body text; the PR lists body-text search as a future possibility and names index bloat vs. stop words as the open trade-off.
- `leads:` Sherlodoc (OCaml docs search) is named as prior art in #144476 ([art-w/sherlodoc](https://github.com/art-w/sherlodoc)); not examined.

## 4. SQLite and Parquet over HTTP Range

**sql.js-httpvfs** `[cited]` ([README](https://github.com/phiresky/sql.js-httpvfs), [blog](https://phiresky.github.io/blog/2021/hosting-sqlite-databases-on-github-pages/)):
- 670 MiB database, 8M+ rows, hosted on GitHub Pages; 1 KiB page size instead of 4 KiB; three virtual read heads with exponentially growing request sizes; an FTS5 table of ~8 MB queried with ~70 KiB fetched; a typical indexed query 130–270 KiB in 10–20 requests; "without matching indices" every non-indexed value can cost a separate request.
- Optional split into fixed-size files (10 MiB in the demo) for host file-size limits and selective CDN caching; `cacheBust` config value appended to URLs; "no cache eviction, so the more data is fetched the more RAM it will use"; the author states it is for small personal projects; last push 2024-08-06.
- FTS5 supports prefix indexes (`prefix='2 3'`) and a trigram tokenizer ([fts5](https://sqlite.org/fts5.html)). Neither gives edit-distance matching.

**Measured sizes** `[measured]`, 100k docs, SQLite 3.50.2:
- Porter+unicode61 FTS5 with `prefix='2 3'`, external-content table, three B-tree indexes: 289.5 MB (`fts_data` 138.5 MiB, content table 127.4 MiB, indexes 8.8 MiB); gzip -9 115.5 MB. Page size 1 KiB vs 4 KiB: +0.01%.
- Trigram tokenizer variant: 508 MB (gzip 234 MB), 1.76× the prefix-index variant.
- `negative:` I did not measure bytes per query; this sandbox has no `strace` and no `/proc/self/io`. The only per-query figures here are phiresky's.

**Does the host honour Range on compressed responses?** `[measured]`, `curl` with `Range: bytes=0-99`, 2026-10-20:

| Host | Result |
|---|---|
| GitHub Pages, `application/octet-stream` (`db.sqlite3.000`, 10,485,760 B) | `Accept-Encoding: gzip`: `206`, `Content-Encoding: gzip`, `Content-Range: bytes 0-99/1484389` (offsets into the gzip stream). `identity`: `206`, `bytes 0-99/10485760`. |
| GitHub Pages, `text/html` | gzip: `bytes 0-99/16162`; identity: `bytes 0-99/109002`. Same pattern. |
| GitLab Pages (`fdroid.gitlab.io`, `inkscape.gitlab.io`) | `200` with the whole body, no `Accept-Ranges`, for gzip, br and identity: Range ignored. |
| Pagefind docs host (Fly.io, not a target) | `206`, gzip offsets compressed; shown only as a third data point. |

- Browsers cannot send `Accept-Encoding: identity`: `Accept-Encoding` is a forbidden request header name ([MDN](https://developer.mozilla.org/en-US/docs/Glossary/Forbidden_request_header)). A 2026 write-up reports the same GitHub Pages behaviour and the SQLite error "database disk image is malformed" ([write-up](https://recca0120.github.io/en/2026/03/07/sql-js-httpvfs-static-hosting/)) `[unverified, secondary]`. phiresky's 2021 demo predates what I measured; a mirrored GitHub Pages issue reports "Length of the file not known" after HEAD returned `content-encoding: gzip` ([mirror](https://identichosting.duckdns.org/github/pages-gem/issues/952)) `[unverified]`.
- GitLab Pages source: `handleContentEncoding` has "don't accept range requests for compressed content" and returns before looking for `.gz`/`.br`; `http.ServeContent` (which handles Range) is used only for seekable disk files, while the zip-VFS path copies the whole file with `io.Copy` ([`helpers.go`](https://gitlab.com/gitlab-org/gitlab-pages/-/blob/master/internal/serving/disk/helpers.go), [`reader.go`](https://gitlab.com/gitlab-org/gitlab-pages/-/blob/master/internal/serving/disk/reader.go), [`serving.go`](https://gitlab.com/gitlab-org/gitlab-pages/-/blob/master/internal/vfs/serving/serving.go)). Matches the measured `200`.
- Pre-compressed files on GitLab Pages: `.br`/`.gz` siblings are served when the browser accepts them, for `.htm .html .xml .txt .text .js .css .svg` only ([GitLab docs](https://docs.gitlab.com/user/project/pages/introduction/)).
- S3: `GetObject` takes `Range: bytes=0-9`, one range per request ([API](https://docs.aws.amazon.com/AmazonS3/latest/API/API_GetObject.html)). CloudFront compresses only listed content types (`application/octet-stream` is not listed) and only objects of 1,000–10,000,000 bytes ([docs](https://docs.aws.amazon.com/AmazonCloudFront/latest/DeveloperGuide/ServingCompressedFiles.html)).
- Limits: GitHub Pages site ≤ 1 GB, soft 100 GB/month bandwidth, soft 10 builds/hour unless a custom Actions workflow ([limits](https://docs.github.com/en/pages/getting-started-with-github-pages/github-pages-limits)); GitHub Pages caches 10 minutes in the responses I saw (`cache-control: max-age=600`) `[measured]`. GitLab.com Pages site ≤ 1 GB ([GitLab.com limits](https://docs.gitlab.com/user/gitlab_com/)); self-managed default 100 MB and 200,000 file entries per site ([admin docs](https://docs.gitlab.com/administration/pages/)); GitLab Pages also sends `max-age=600` `[measured]`.

**DuckDB-WASM / Parquet** `[cited]`
- DuckDB reads a Parquet file partially through its metadata plus HTTP range requests ([httpfs](https://duckdb.org/docs/current/core_extensions/httpfs/https)). Measured by a third party: `create view` over a remote Parquet transfers 37 KiB; `count(*)` ~114 KiB; a column sum over one 159 MB file 5.3 MB ([Willison](https://til.simonwillison.net/duckdb/remote-parquet)).
- When the server does not honour Range, DuckDB downloaded the whole 17 MB file in a reported case ([Observable forum](https://talk.observablehq.com/t/do-parquet-files-in-duckdb-connections-use-http-ranged-requests/7615)) `[unverified, old]`.
- `negative:` no inverted index inside Parquet; text search is a scan of the selected columns (row-group min/max pruning helps `name >= 'x'` and `kind = 'agent'` only if the file is sorted that way). I found no source measuring DuckDB-WASM substring search over 100k text rows.
- `leads:` the same GitHub Pages gzip behaviour applies if `.parquet` is served as `application/octet-stream` (not measured on a `.parquet` file).

## 5. Docs-site and package-catalog patterns

- **MDN** `[cited]`/`[measured]`: autocomplete loads one `search-index.json` per locale (titles + URLs, sorted by page views), builds a FlexSearch index in memory, deferred until focus; 11,690 titles "only 144KB over the network" in 2021 ([Mozilla Hacks](https://hacks.mozilla.org/2021/08/mdns-autocomplete-search/)). Today `en-US/search-index.json` has 14,706 entries, 1.4 MB raw, 226 KB gzip, served with `accept-ranges: bytes` ([endpoint](https://developer.mozilla.org/en-US/search-index.json)). Full-text site search is a separate backend, described as Elasticsearch-style ranking in the same post. Title-only data is the reason a single file works at this size.
- **Material for MkDocs** `[cited]`: one lunr index (`search_index.json`) fetched on first search; v9 index 573 kB to 335 kB and an 8.2 MB corpus to 4.4 MB; the author calls an 800,000-word corpus "a size most documentation projects will likely never reach" ([blog](https://squidfunk.github.io/mkdocs-material/blog/2021/09/13/search-better-faster-smaller/)). A very large `search_index.json` made an Apache Pekko docs search sluggish; that file came from Paradox, not Material ([issue mirror](https://github-redirect.dependabot.com/apache/pekko/issues/1097)) `[unverified, mirror]`.
- **Docusaurus** `[cited]`: `docusaurus-search-local` ships lunr indexes, one per search context path if configured, with content-hashed filename or query ([README](https://github.com/easyops-cn/docusaurus-search-local)).
- **Homebrew formulae.brew.sh** `[cited]`/`[measured]`: Jekyll static site on GitHub Pages; `api/formula.json` is 32.2 MB raw with `accept-ranges: bytes`; site search is Algolia DocSearch with a facet filter, indexed by an Algolia crawler ([`_layouts/base.html`](https://github.com/Homebrew/formulae.brew.sh/blob/master/_layouts/base.html), [README](https://github.com/Homebrew/formulae.brew.sh/blob/master/README.md)). `negative:` needs a hosted service for search.
- **Nixpkgs search** `[cited]`: Elasticsearch/OpenSearch backend ([nixos-search README](https://github.com/NixOS/nixos-search)). `negative:` needs a server.
- **crates.io sparse index** `[cited]`: static files at a name-derived path (`1/`, `2/`, `3/<c>/`, `ab/cd/<name>`) for exact-name lookup, no search ([registry index](https://doc.rust-lang.org/cargo/reference/registry-index.html)). This is a prefix-shard layout for key lookup, not an inverted index.
- `negative:` crates.io, npm, pkg.go.dev, Flathub style catalog search I did not find documented as static-hosted; I did not check each individually.

## 6. Hand-built term-prefix shards

- **First-letter shards** `[cited]`: 26 JSON files, one per initial letter, word to document ids; 8 MB index, largest shard ~800 KB; suggests two-letter keys if still too big; words under three letters skipped; a separate trie for autocomplete; multi-term queries fetch one shard per term and intersect ([MacWright 2012](https://macwright.com/2012/11/14/indexing-searching-big-static-data)).
- **Shard-count arithmetic from shipped designs** `[derived]`: Pagefind built 225 chunks for 99,142 words (avg ~440 words/chunk) on 29,977 pages ([Pagefind#49](https://github.com/Pagefind/pagefind/issues/49)), 27 chunks for 22,852 words on 2,496 pages ([intro](https://github.com/Pagefind/pagefind/blob/main/docs/content/docs/_index.md)); my build gave 999 chunks / 218,634 words / 100,000 pages. Chunk count tracks postings volume (≈ one chunk per 20000 locations), roughly 1 chunk per 100 documents here, not vocabulary.
- **Skew** `[measured]`: Pagefind's largest chunk at 100k is 616 KB against a 28 KB median, because packing is by postings count and a common word's posting list cannot be split. A fixed one-letter or two-letter key has the same skew problem (MacWright's 800 KB largest shard against an 8 MB total / 26 average of ~300 KB).
- `negative:` I found no documented hash-bucket (term-hash) sharding design for static search. Hash naming exists only for cache busting.
- Forum proposal, untested: treat each prefix as its own Pagefind site and merge them in the browser ([Hugo forum](https://discourse.gohugo.io/t/scaling-pagefind-to-over-1-million-pages/56109)).

## 7. Facet-only browse without loading everything

- **Pagefind:** needs the whole word index for a null term (measured above). `filters()` for counts costs the filter files only (1.7 MB at 100k). `leads:` untested idea, a token present in every document would make the "term" resolve to one chunk, at the price of a posting list as long as the catalog.
- **SQLite over Range:** natural fit: `WHERE kind = ? ORDER BY name LIMIT 20` walks one B-tree range; keyset pagination (`name > ?`) avoids OFFSET scans; facet counts via `GROUP BY` need a covering index or a precomputed counts table, otherwise a full scan ([blog](https://phiresky.github.io/blog/2021/hosting-sqlite-databases-on-github-pages/): "without matching indices" costs a request per value). Not measured here.
- **Parquet:** sort the file by `(kind, name)` so row groups prune; paging is by row-group range. Not measured.
- **Whole-index loaders:** browse is a filter over the in-memory documents; cost is the full load above.
- **Pre-rendered pages (no search library)** `[cited]`: Homebrew's `formula_index.html` and per-formula JSON are plain static files ([repo](https://github.com/Homebrew/formulae.brew.sh)); paging through a sorted listing as static page files is the zero-index option. `negative:` I found no source with paging numbers for it.

## 8. Index that changes every hour

| Approach | Invalidation unit | Mechanism | Incremental? |
|---|---|---|---|
| Pagefind | per file | hashed names; unhashed `pagefind-entry.json` | no; full re-read, hash reuse only |
| stringdex | per tree node | Merkle hash names | no published incremental path |
| Whole-index loaders | whole file | filename hash or query param (Docusaurus `hashed`) | no |
| sql.js-httpvfs | whole DB, or per 10 MiB chunk | `cacheBust` query value; "caching-related database corruption" if mixed versions | no; B-tree pages shift on rewrite |
| Parquet | per file | replace file; row groups cached by URL | append by adding files |
| GitHub/GitLab Pages | whole site | `max-age=600` on both | atomic deploy |

- `negative:` a mixed-version read (old meta, new chunk) is only prevented by hashed names; sql.js-httpvfs documents the corruption risk and gives `cacheBust` as the mitigation.

## Measurement method

- Corpus: N synthetic HTML pages generated from 673,664 words of this repository's markdown (vocabulary 22,366); each page 120–200 contiguous words (random window) plus a unique name token, `kind` (6 values, skewed), `namespace` (300 values), `license` (8 values). 100,000 pages = 146.4 MB; the rebuild probes and the Orama ratio used a smaller corpus of the same shape. Unique name tokens inflate vocabulary relative to prose; term frequencies follow this repo's text, so the word `skill` matches 56% of documents, which makes short-term CPU numbers pessimistic.
- Pagefind: release binary v1.5.2, sha256 matched the published file. Bytes per query: local static server logging response sizes of files requested by Pagefind's JS API running in Node (cold = fresh instance); `pagefind.js` (45.5 KB) is not counted in the tables.
- Whole-index loaders: `npm i --ignore-scripts`; heap = `process.memoryUsage().heapUsed` after forced GC; compression via Node zlib (gzip level 9, brotli default).
- SQLite: Python `sqlite3` 3.50.2; sizes from the file and `dbstat`.
- Host Range behaviour: `curl` `Range: bytes=0-99` with different `Accept-Encoding` against live sites on 2026-10-20.
- Scripts and corpora were scratch files, not committed.

## Open items

- `leads:` measure bytes per query for SQLite FTS5 and Parquet with a real Range tracer (the sandbox had none).
- `leads:` verify `.parquet` and `.sqlite` Content-Type and compression on GitHub Pages with a throwaway Pages site; only existing sites were probed.
- `leads:` read stringdex's size and fetch behaviour on the wordnet dictionary test (150k+ words) as the closest published static name-search at 100k scale.
- `leads:` browser (not Node) memory and worker timing for Pagefind at 100k.
