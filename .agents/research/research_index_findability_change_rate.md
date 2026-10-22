# Research: Index findability and change rate for large indexes

## Metadata

- Date: 2026-10-17
- Domain: cli
- Triggered by: discussion .agents/discussions/index-scale-rollout.md
- Expires: 2027-04-17
- Repos read: `grimoire` (`src/catalog`, `src/command/search.rs`, `src/tui`), `grimoire-indexer` (`src/data`, `src/enrich`, `src/ratings`, `src/downloads`, `src/validate`, `src/ci.ts`), `grimoire-index` (`.github/workflows`, stale local `dist/`), `grimoire-vscode` (`src/webview`, `src/views`, `src/grim.ts`)
- Out of scope: transport size, 500-entry cap measurement, gzip/ETag (settled earlier). No network was used; nothing here is measured against a live index.

## Findings

### 1. Findability

**Ranking and cut.** One matcher serves CLI, MCP and TUI (`src/catalog/search_match.rs`).
- Fuzzy (skim, floor 12 per char) on repo leaf x3 and keywords x2, with an exact-name bonus. Word-prefix on summary x2, description x1 and namespace x1. Kind is an exact filter. AND across terms; the score is the sum of per-term bests (`search_match.rs:93-107`, `:109-133`, `:300-340`).
- `retain_relevant` drops rows scoring under `search_min_relevance` percent (default 50) of the best row (`search_match.rs:150-159`; default at `src/config/defaults.rs:28`). `--sort` replaces relevance after the cut (`src/command/search.rs:332`, `order_results`).
- Hidden rows are reported on stderr only, never in JSON (`search.rs` `hidden_rows_notice`).
- Not searched: version `tags`, the index `owner`, the index namespace (`CatalogEntry.index_path`). `score_fields` takes kind, repo, summary, description and keywords only (`search_match.rs:240`).
- The design assumed small catalogs: "at most a few thousand rows ... linear scan" (`.agents/adr/adr_search_relevance_model.md:46-47`, `:100`). The 20,000-entry design point is above that stated assumption.

**What narrows today.**
- A bare kind word in the query is an exact filter; there is no `--kind` flag (`search_match.rs:9-16`).
- `--registry` collapses the browse set. `--show-deprecated` is off by default; deprecated rows hide unless installed (`search.rs:419`, `deprecated_row_visible`).
- Per `[[registries]]` entry `include`/`exclude` globs act on repo path or the full ref. The same locator can be declared twice as two named views (`catalog_service.rs:1239`, `adr_registry_browse_filters.md`, `adr_registry_filter_match_candidate.md`).
- `--sort name|updated|rating|downloads`.
- Not available: filter by owner, license, rating floor, or tag. There is no limit or pagination flag on `grim search`.

**The cap precedes every narrowing step.**
- The catalog is built under the empty scope (`catalog_service.rs:14-20`, `:364`). `build_from_index` takes the first `MAX_CATALOG_REPOS + 1` entries in `all.json` order and truncates to 500 (`registry_catalog.rs:828-834`). The query, `include`/`exclude` and `--sort` all run later, over those 500.
- `all.json` order is path-sorted (`grimoire-indexer/src/data/index.ts:107-109` via `findMetadataFiles(...).sort()`, written at `:169`). So the visible 500 are the first 500 pointers by `index/<host>/<namespace>/<pkg>` path.
- The docs say it outright: the cap is applied "before the filter — a narrow filter can never widen the window" (`.claude/rules/subsystem-cli-commands.md`, `grim search` row).
- Consequence, from the code: at the 20,000-entry design point no query or filter reaches pointers past that window, whatever they match.
- Truncation is surfaced weakly. `grim search` warns on stderr only for a non-empty query (`search.rs:173`). The JSON report is `{items, sources}`, and `SearchSourceStatus` has no `truncated` field (`src/api/search_report.rs:247-254`). The TUI shows "(list truncated)" (`src/tui/render.rs:1102-1105`). The VS Code extension does not read a truncation signal (`grep truncated` in `src/` finds only `FetchResult`).

**TUI.**
- Default view is Tree, `group_by_type` off, `expand_levels = 1` (`defaults.rs:16-21`, `:43`), so it opens with only registry roots expanded. An index row groups by the index `host/namespace/name`, not its OCI path (`tui/tree.rs`, `CatalogEntry.index_path`).
- Holds up: the grouped, collapsed-by-default tree.
- Does not hold at scale (unmeasured, from structure only):
  - `recompute_filter` rescans every row with full skim on each query edit (`tui/state.rs:2088-2105`).
  - `frame()` projects every filtered row into a `RenderRow` each frame, and the renderer builds one `ListItem` per row (`tui/render.rs:899-917`, `:1330`; the `List` widget is windowed by ratatui but the input vector is not).
  - I found no debounce on query input.

**VS Code.**
- Search runs `grim search -- <query>` after a 250 ms debounce (`webview/sidebar/main.ts:912`, `src/grim.ts:699-717`). Flags used: `--refresh`, `--show-deprecated` only. It never passes `--sort`, `--registry` or a kind.
- Relevance order is discarded. Browse sorts client-side with default `downloads desc`, then `updated` (from `created`), then name (`webview/model.ts:793-798`, `:864-869`). The comment at `model.ts:807-815` says the relevance mode was dropped. On indexes without Artifactory counters, downloads are null everywhere, so the order falls through to date then name.
- No pagination and no virtualisation. Cards go through lit `repeat()` over the whole filtered array (`webview/render.ts:815-850`). The IntersectionObserver only reports the viewport for prefetch, capped at 500 repos (`views/sidebar.ts:84`, `:448`; `webview/sidebar/main.ts:416`).
- Kind chips are client-side only (`model.ts:785-916`). The only grouping is none or by-registry (`model.ts:1018-1023`).
- Not carried to the extension: `grim search` JSON has no `keywords` field and no owner (`src/api/search_report.rs:164-213`), so the extension cannot filter or facet by either.
- Holds up: grim does the query work, and the debounce limits spawns.
- Does not hold up: relevance order is lost, nothing is paginated, and every card is rendered at once.

### 2. Duplicates

- **Same `ref`, different pointer paths.**
  - Neither the contribution gate nor the compiler checks `ref` uniqueness across pointers. The gate checks path shape, owner login and id against the namespace, and that the ref's tags are reachable (`grimoire-indexer/src/validate/index.ts:205-265`). The compiler only dedupes namespaces (`src/data/index.ts:179`). A negative grep for uniqueness checks came back empty.
  - The gate does not tie the ref's registry path to the announcing namespace. Any reachable ref passes.
  - grim silently collapses same-ref rows. `build_from_index` collects into `BTreeMap<repo(), CatalogEntry>`, so the later pointer in path order wins (`registry_catalog.rs:836`). `take(501)` runs before that collapse, so duplicates consume cap slots (`:830-834`).
- **Same repo from two configured sources** is deliberately kept as separate rows, with a `source` attribution (`tui/state.rs:1045-1048`, S-022b). The extension keys a card on (source, repo) and keeps both (`webview/model.ts:427-461`).
- **Near-duplicates** (same name in different namespaces, forks, renamed copies): nothing detects or collapses them. `hex` appears under more than one namespace in the local dist and both show.
- The stats sidecar is keyed by ref, so two pointers to one ref share one rating and one download count.

### 3. Change rate

**Cadence.**
- `pages.yml` (generated from `src/ci.ts`, rendered in the index repo) triggers on every push to `main`, on `workflow_dispatch`, and hourly `23 * * * *` (`.github/workflows/pages.yml:28-31`; `STATS_CRON` at `grimoire-indexer/src/ci.ts:266`). The schedule is rendered only when `ratings` or `downloads` is configured (`ci.ts:375`).
- `refresh.yml` in the index repo is that repo's own policy: a daily 04:17 dispatch of `pages.yml` (`.github/workflows/refresh.yml`).
- Every `pages` run executes `npm run enrich -- --seed`, then `build`, then deploys. With `autoMerge: true` every merged announce PR is a push, so every announce is a full rebuild (`index.config.json`, `pages.yml`).
- `enrich` loops sequentially, one package at a time. Per package it always runs `grim describe`, plus `fetch --description --digest-only` when a companion exists, plus a `fetch` when the digest moved (`src/enrich/index.ts:330-350`, `enrichOne`). The code comment reads "batch with a worker pool if an index ever grows into the hundreds". At the 20,000-entry design point that is 20,000 or more sequential subprocess calls per build.

**Checkpoint cap.**
- `enrich --seed` restores state from the published `enrich.json`, which holds every sidecar plus base64 README, contents and logo (`src/enrich/checkpoint.ts:112-155`). The reader refuses anything over `LARGE_RESPONSE_BYTES` = 16 MiB (`src/validate/adapters/http.ts:27`, `checkpoint.ts:150`, `:171`). The writer warns "published but not read back".
- Sample only: the local stale `grimoire-index/dist/enrich.json` is 282,492 bytes for 12 enriched packages, avg 23.5 KB, range 11.7-40.3 KB. Extrapolating, 16 MiB is reached near 700 packages (16,777,216 / 23,541). That is arithmetic on a 12-package, 10-day-old build that predates `contents.md` sidecars, so the real figure is likely lower. It is not measured against a current build.
- When the seed is not read, `enrichOne` has no `existing.contentDigest`, so `moved` is true and `data.updated = now` for every artifact that carries no `created` (`enrich/index.ts:317-318`). The `checkpoint.ts` header states the same: "re-stamped `updated` on every artifact that carries no `created` of its own".

**Per-record fields in `all.json`.** A record is `{...enrich sidecar, ...metadata.json}`; `metadata.json` wins on overlap (`src/data/index.ts:131-145`).
- *Static between rebuilds.* `name`, `kind`, `ref`, `owner`, `namespace`, `repository`, `description`, `summary`, `keywords` (and `license`, `created`, `deprecated`, `replaced_by` when announced). They change only on a re-announce, which is a PR and so a push.
- *Change on a publisher release, no announce needed* (picked up by the next enrich): `version`, `tags` (appended; floating tags keep their names), `created`, `updated`, `contentDigest`, `hasContents`, `title`, `license`, `authors`, `vendor`, `url`, `documentation`, `support`, `deprecated`, `replacedBy`, `hasReadme`, `hasChangelog`, `logo`. The list is `META_KEYS` plus bookkeeping in `enrich/index.ts:90-99` and `:289-323`. `descDigest` is stripped before publishing (`data/index.ts:139`); `contentDigest` is not.
- *Change with no publisher action.* `updated` re-stamping when the checkpoint seed is unreadable (above). Also a failed describe keeps the old sidecar ("stale beats empty"), so a transient failure shows as no change, not as churn.

**Estimate, hedged.**
- On the stale 19-record local `dist/all.json` (22,032 B on disk, 19,343 B counted as compact key/value), the release-volatile fields `tags` + `version` + `created` are 6.6% of bytes (4.5% + 1.1% + 1.0%; `description` is 42.5% and static).
- That build predates `updated`, `contentDigest` and `hasContents`. Adding them at roughly 36 + 95 + 22 B per record on a ~1,018 B average record gives about 15% more, so at most ~22% of a record is release-volatile. This is arithmetic, not a measurement of a current file.
- With the seed working, bytes changing per rebuild are about (fraction of records released since the last build) x (<= ~22% of a record). A rebuild with no releases changes none of `all.json`.
- Without a readable seed, add `updated` (~36 B / ~1,018 B = ~3.5%) on every record that lacks `created`.

**`stats.json`.**
- `generated_at` is restamped on every collector run: `statsDocument(...)` uses `now` (`src/ratings/seed.ts:196-200`). `renderer/index.ts` carries an existing value over rather than re-stamping.
- Each ref's `downloads.as_of` is stamped from the run clock for every ref the collector reads (`src/downloads/artifactory.ts:42`, `:227`). On an Artifactory-backed index with downloads enabled, every counted ref's entry changes at least in `as_of` on each hourly run, even when `total` did not. `rating.up` changes only on votes. `updated.at` is stable.
- grim reads `stats.json` in a second request beside `all.json`, and ignores the `updated` key entirely (`WireStats` in `src/catalog/index_source.rs:222-228`). Its only date is `all.json` `created`, which is absent for records without git provenance (5 of 19 in the sample).

### 4. Usage and ownership signals on records

On `all.json` records (current indexer, from `META_KEYS` and `IndexRecord`):
- Ownership: `owner {github|login, id}`, `namespace`, `repository`, `authors`, `vendor`, `url`, `documentation`, `support {issues, chat, contact, security}`, `license`.
- Lifecycle: `deprecated`, `replaced_by` (announce, snake_case) and `replacedBy` (enrich, camelCase), `version`, `tags`, `created`, `updated`.
- Usage, from `stats.json`: `rating.up` + opaque `url`/`target` (GitHub Discussions upvotes, GitLab award emoji), `downloads {total, versions, as_of}` (Artifactory only; null on GHCR and GitLab registries, per `index_source.rs:261-280`).

What reaches `grim search --format json` for index-backed rows (`index_source.rs:92-155`, `search_report.rs:164-213`):
- Reaches it: `kind`, `repo`, `source`, `description`, `summary`, `repository`, `created`, `deprecated`, `replaced_by`, `rating {up, url}`, `downloads`, and `oci.licenses`. `version`, `latest_tag` and `revision` are always null on these rows.
- Never reaches it: `owner`, `authors`, `vendor`, `url`, `documentation`, `support`, `keywords`, `tags`, `updated`. `IndexPackage` does not declare them and serde ignores the unknowns. Authors, vendor, url and documentation exist in grim only via a live `grim describe` or from OCI `_catalog` rows.
- `owner.id` and `login` are used by the gate for authorization only, not by any reader.

## Negative

- No dedupe or uniqueness check on `ref` anywhere in the validator, compiler or grim, beyond the `BTreeMap` collapse (`registry_catalog.rs:836`).
- The local `grimoire-index/dist/` is a stale dev build (all.json 2026-10-07, 19 records, 12 enriched, no `updated`/`contentDigest`). Field-share numbers come from it; it is not the live site. The live `all.json` could not be fetched (no network).
- The 23.5 KB per-package checkpoint average rests on 12 packages; the ~700-package crossover is extrapolation.
- No benchmark, benches directory or scale test exists for search or the TUI (`ls` of repo root; the ADR only states an assumption).
- No evidence found of a grim-side or indexer-side search over `tags` or `owner`.
- TUI `recompute_filter` and `frame()` costs at 20,000 rows were not measured; the "does not hold" call rests on the code structure only.
- `refresh.yml` (daily) is redundant with the hourly schedule whenever `ratings` or `downloads` is configured; it matters only for an index that has neither.

## Leads

- Cap window bias: the visible 500 are the alphabetically first by `index/<host>/<ns>/<pkg>`. Compare what a lexicographic window does to namespaces late in the alphabet.
- `enrich.json` checkpoint: confirm size per package on a current build, and what a >16 MiB seed means for `updated` churn and build time.
- Sequential enrich at 20,000 entries: wall time per build against the Actions job limit, and how pushes queue behind `cancel-in-progress: false`.
- `replaced_by` (announce) vs `replacedBy` (enrich): grim reads only the snake_case key, so the enrich-sourced successor never reaches `grim search`.
- Ref/namespace binding: the gate proves namespace ownership and ref reachability but not that the ref belongs to the announcer. Relevant to duplicate pointers and impersonation.
- VS Code ordering: with a query typed, the extension replaces relevance with downloads/date order. Needs an owner-level call, not research.
- `grim search` has no `truncated`, `limit`, owner or keyword fields in its JSON; adding them is additive under Principle 9.
- Per-source browse filters can slice one index into named views by namespace glob, but only inside the 500 window.
