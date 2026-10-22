# Research: Index scale and freshness — decision archaeology

## Metadata

**Date:** 2026-10-17
**Domain:** cli
**Triggered by:** discussion `.agents/discussions/index-scale-rollout.md`
**Expires:** 2027-04-17

Read-only archaeology over `grimoire` (grim CLI) and `grimoire-indexer`. No
recommendation. Facts verified against the working tree and `git log` on
2026-10-17; line numbers are as of that date.

## Research: index-scale-archaeology

### 1. `adr_catalog_freshness_revalidation.md`

**Status:** Proposed, 2026-08-11. Introduced and parked in one commit,
`29ded5af` `chore(agents): park catalog-freshness ADR, plan and research`
(2026-08-12), whose body says "Planning artifacts only — no source change,
nothing implemented". The file has never been edited since (`git log` shows
that single commit).

**What it decides (D1..D8, as drafted 2026-08-11):**

| # | Title (ADR heading) | Fate after the 2026-08-12 Revision block |
|---|---|---|
| D1 | TTL semantics: two clocks, per-source-kind floor, finite ceiling (`INDEX_TTL_SECONDS = 300`, `MAX_STALE_MULTIPLE = 3`) | Superseded in part: floor survives, **validator-gated** (`validator.is_some()` picks 300 vs 3600); ceiling withdrawn |
| D2 | `grim status --check` (and every one-shot) opts out of stale-serving | Withdrawn entirely (no `Freshness` enum) |
| D3 | `load_catalog` params-struct collapse (`CatalogRequest`) | Withdrawn |
| D4 | On-disk format: three additive fields (`CatalogFile.validator: IndexValidator{etag,last_modified}`, `CatalogFile.git_tip`, `CatalogEntry.digest`); wedge fix is a hard precondition; `fetch_http` gets `Policy::limited(5)` and `INDEX_BODY_LIMIT = 32 MiB` via the `CappedSink` idiom | Stands with 2 corrections (304 restamps entries' `fetched_at` too; failed rebuild falls back to the cache already read). Plan later dropped `git_tip` and `digest` |
| D5 | Seam migration: one projection, two producers, offline from context (TUI catalog load async) | Stands, promoted to primary delivery of the reported defect |
| D6 | Cancel-on-move focused-row refresh, in-memory overlay | Stands |
| D7 | Hardened `ls-remote` probe + hardened conditional clone + config-time locator shape gate (`GIT_ALLOW_PROTOCOL=file:http:https:ssh:git`) | Stands; plan dropped the `ls-remote` probe but kept hardening of the clone and the announce path |
| D8 | Migration and rollout; waves 1-4; no config knob, TTLs stay compiled-in constants ("rejected as YAGNI, named so it is not re-proposed") | Stands minus withdrawn pieces |

**Central reversal (Revision 2026-08-12, ADR lines 48-133):** the original
decision O3 (two clocks + caller-declared `Freshness` policy) was withdrawn by
owner ruling after the adversarial review re-derived the weighted matrix; three
of 18 cells were wrong, re-derived totals **O1 116 / O3 112**. Adopted O1+O5:
TTL stays a blocking gate, revalidation made cheap, TUI catalog load made async.
Stated problem (plan, "Problem in one line"): publish, wait three minutes, open
the TUI, change not visible; the TUI blocks on `reload_into`
(`src/tui/app.rs`) during a rebuild.

**Why still Proposed — what the record says and does not say:**

- Recorded: the plan header says the ADR is "Status: Proposed - the owner has
  not accepted it, and execution should not begin until they do"
  (`plan_catalog_freshness_revalidation.md`, Status block). `hex.md` (line ~304)
  lists it as "Parked plan ... `Step: /hex-execute` - not started". The parent
  `meta-plan_promotion_1_0.md` was marked "suspended; resume after this plan"
  (`hex.md`); its own Status block still reads "Phase 3 - every workstream marked
  'Blocks 1.0: yes' is done".
- Recorded gaps at park time: the cross-model adversary gate "did not run - it
  produced no output" (plan Status; `hex.md` "Adversary learning, 2026-08-12");
  3 open questions in the ADR (`## Open questions`): (1) no live
  `curl -sSI https://index.grimoire.rs/all.json` to confirm `ETag`/
  `Last-Modified` survive the CDN ("run the spot-check before wave 2 ships");
  (2) OCI fan-out 429 behaviour unmeasured; (3) focused-row tag-cache churn
  unmeasured.
- **Not recorded anywhere I found:** an explicit reason for the park beyond
  "owner has not accepted". No review, discussion, retro-inbox entry or
  memory note (`~/.claude/projects/.../memory/MEMORY.md` index has none) says
  why it stalled. The only circumstantial context: the next work item after
  2026-08-12 was the materialization-drift fix loop (finalized 2026-08-13,
  its plan says "Parent plan: meta-plan_promotion_1_0 (parked, alongside
  `plan_catalog_freshness_revalidation`)") and then the ratings ADR
  (2026-08-18), which cites this ADR with "read its `Revision 2026-08-12`
  block, not the original proposal" (`adr_artifact_ratings.md` ~line 1296).
- Also referenced as live-but-unbuilt by `research_rating_architecture_map.md`
  ("Freshness: what is actually in force": "Proposed, unimplemented. Zero
  ETag/If-None-Match/If-Modified-Since occurrences anywhere in `src/`") and
  `research_repo_reverification_history.md`. `arch-principles.md` ADR index row
  carries "Proposed, not accepted".

**Implemented vs not (grep of `src/`, `Cargo.toml`, `test/`, `docs/src`):**

| Item | State |
|---|---|
| `IndexValidator`, `INDEX_BODY_LIMIT`, `INDEX_TTL_SECONDS`, `If-None-Match`, `ETag`, `Accept-Encoding`, `gzip`, `304`, `git_tip`, `MAX_STALE_MULTIPLE`, `freshness_window` | **Zero hits** in any commit (`git log --all -S` on each: empty) |
| `fetch_http` (`src/catalog/index_source.rs:585-605`) | Unchanged shape: plain `client.get(&url).send()`, `error_for_status`, unbounded `.bytes()`, `serde_json::from_slice`; no redirect policy set; timeout is a single total `HTTP_TIMEOUT = 30 s` (`:34`) |
| reqwest features | `Cargo.toml:58` `reqwest = { version = "0.13", default-features = false, features = ["rustls"] }`: no `gzip` feature |
| TTL | Single `CATALOG_TTL_SECONDS = 3600` (`src/catalog/registry_catalog.rs:39`) for every source kind |
| Wedge fix (ADR wave 1: unreadable cache = cold cache) | **Landed incidentally** in `535b27c9` (2026-08-19, ratings sidecar, "S-015"): `Catalog::load_or_cold` (`registry_catalog.rs:520`), used in `coordinate` (`:713`, `:743`). ADR/plan C-007 satisfied in effect; no commit cites the freshness ADR. `deny_unknown_fields` is **still on** `CatalogEntry` (`:244`) and `CatalogFile` (`:361`) plus `RatingSummary`/`DownloadSummary`, so the ADR's drop-it clause is unimplemented and any new field still makes an older grim reject the cache (now cold-rebuilt, not wedged) |
| Failed-rebuild falls back to read cache (ADR "correction 2", plan C-032) | **Not done**: `build_from_index` error arm still `dispose_guard` then `return Err` (`registry_catalog.rs:685-688`; same for the `_catalog` path `:648-650`). The ratings sidecar has its own carry-forward on failed fetch (`535b27c9`), but that covers `stats.json` only |
| Async TUI load (D5/WP-1), focused-row refresh (D6), git hardening incl. `GIT_ALLOW_PROTOCOL` (D7/WP-7,8), locator gate | Not found in git history (not exhaustively verified for the TUI; `git log -S` for the new type names is empty) |
| Plan WP status | All 8 WPs unstarted (plan Status block: "Active phase: 0 - Planned, not started") |

Plan scope after owner rulings (plan "Scope cuts", 2026-08-12; the plan is
"authoritative wherever it and the ADR disagree"): dropped the OCI HEAD digest
gate + `CatalogEntry.digest` and the `git ls-remote` probe (git revalidation
remains a full `--depth 1` clone); index floor 300 s dropping to 60 s only once
a 304 has been **observed** from that host; OCI stays 3600; `grim search`
revalidation deadline-bounded (VS Code drives it as interactive browse); `grim
status --check` keeps the unbounded blocking rebuild.

**Scale assumptions in this ADR:** none about entry count. `all.json` size is
never stated; the only size figure is the `32 MiB` body cap, and the research
notes `Accept-Encoding`-coupled validators (`"<hash>+gzip"`) as a caveat
(`research_catalog_revalidation_http.md` §1). Research rationale for conditional
GET is latency/politeness against GitHub Pages (Fastly), not payload size.

### 2. Other grimoire `.agents` documents touching these topics

| Doc | Size/scale assumption | Explicitly deferred |
|---|---|---|
| `MAX_CATALOG_REPOS = 500` itself (`registry_catalog.rs:40-50`, introduced `bf267f54` 2026-05-15 "catalog + TUI (phase 6)" for the OCI `_catalog` walk: "tens of thousands of repositories ... three round-trips ... infeasible"; "walking the entire registry is an explicit cut-line") | Registry may have 60k repos; browse is a bounded window | Walking whole registry |
| Cap copied onto index sources in `56d72a92` (2026-07-02, "package-index browse sources"): `build_from_index` does `.take(MAX_CATALOG_REPOS + 1)` / `truncate` (`registry_catalog.rs:831-834`) with doc "same prefilter and cap as the `_catalog` walk" | No stated rationale in the commit body or in `docs/src/content/docs/package-index.md` for applying a per-repo-round-trip cap to a single static file | n/a |
| `91b39070` (2026-06-11) `fix(search): longest-term prefilter and visible catalog truncation` | Added `truncated()` + stderr warning; "an empty query is an explicit browse and the cap is the documented cut-line - no warning" (`src/command/search.rs:~170`) | Warning only on non-empty query |
| `adr_registry_browse_filters.md` D6 (lines ~345-367) + `plan_registry_browse_filters.md` C-008 | Filter is read-time only; "cap is applied while listing, before the filter is ever consulted"; a narrow filter **cannot rescue** a browse from the 500 cap; documented with number 500 in `configuration.md:597`, `commands.md:1221` | Build-time prefilter rejected (shared on-disk cache keyed by registry url would poison `status --check`); params-struct collapse deferred |
| `handover_registry_set_review.md:333` | Measured 174 us cloning a `Catalog` at "the 500-entry `MAX_CATALOG_REPOS` cap" (1.25 % of a 13.9 ms `grim search`) | Fix suggested, not applied |
| `adr_search_relevance_model.md` (2026-09-25) | "the catalog is at most a few thousand rows, matched in-process on every keystroke in the TUI"; no index/build step; "Per-keystroke cost ... description scoring dominates at 5k rows" | **Key coalescing "deferred until it is measured as a problem"**; typo tolerance/BM25/nucleo deferred; the relative cutoff makes "what a query returns depend on the best hit in the catalog" |
| `adr_artifact_ratings.md` "Scale decision, with measurable expiry" (~line 852) | Forge-as-database comfortable to **~1-2k artifacts**, catalog "~200 today"; 10k cold-start thread creation = ~20 h under GitHub's 80/min-500/h limit, drained by a 400/run budget in ~25 runs; `stats.json` ~75 KB projected at 10k artifacts / 5 % participation, **revisit trigger at ~200 KB gzipped** (~25-30k rated entries) | Pagination/query API for `stats.json` beyond the trigger; `stats.json` is fetched in the same pass as `all.json`, cached under the same envelope, "at most hourly per source" |
| `specs/design_artifact_ratings.md:133` | Build job "seconds at 200 artifacts, still seconds at 10k" | n/a |
| `research_rating_operability.md` | 10k artifacts examined for CI cost; "rate-limit wall clock problem, not CI-minutes" | Cursor optimization for re-listing |
| `adr_default_provenance_and_support_channels.md:176` | "`all.json` is the only browse-time source", hence pointers gain `license`/`created` | Every pointer field enlarges the frozen `all.json` |
| `handover_indexer_annotation_fields.md` §4 | `all.json` pointers gain `license`+`created`; "a backfill pass over `enrich/` would populate the rest sooner" | Backfill |
| `plan_tui_tree_index_layout.md` (L56, L128) | `index_path` derived from `all.json` `namespace`+`name` (HTTP) or directory path (git); OCI `_catalog` sources unchanged | Flat list unchanged |
| `docs/src/content/docs/stability.md:40`; `package-index.md:151-157` (spec "Compiled Artifacts") | `/all.json` "one JSON array" and "the only endpoint grim's HTTP transport requires"; **transport locators are frozen**: HTTP `<base>/all.json`, optional `<base>/stats.json`, git `index/<host>/<ns>/<pkg>/metadata.json` | Stability §: "evolution is additive-only"; row *set* returned is not a contract (`adr_search_relevance_model.md` Principle 9 ruling) |
| `specs/design_brief_landing_page.md:111` | Public index "holds 12 packages, all first-party. Never imply a populated ecosystem" | n/a |
| `adr_multi_registry_mcp.md` | Per-registry cache `$GRIM_HOME/catalog/<hash>.json`, `AdvisoryFileLock`, serve-stale-on-contention; no size discussion | MCP in-memory cache retired by freshness ADR |
| `research_stale_while_revalidate_ux.md`, `research_catalog_revalidation_http.md` | Latency/rate-limit framing; no catalog-size figures | Live header spot-check never done |

No grimoire ADR/plan/research (other than the two untracked 2026-10-17 research
files for the current discussion) states an expected `all.json` byte size,
entry count ceiling for index sources, gzip support, or sharding.

### 3. Indexer (`/home/mherwig/dev/grimoire-indexer`)

**16 MiB constant.** `LARGE_RESPONSE_BYTES = 16 * 1024 * 1024`
(`src/validate/adapters/http.ts:27`). Introduced by `d6305d0` (2026-08-28
`fix(ratings): stop reading a large page as a transport failure`): `request()`
capped every response at `MAX_RESPONSE_BYTES = 1 MiB` and returned `{status:0}`
on overflow, indistinguishable from a transport error; the ratings GraphQL page
(`PAGE_SIZE` 50 threads with full bodies) overran it. The per-call cap was set
to "16 MiB, the ceiling `enrich` already accepts from one `grim` subprocess"
(`MAX_OUTPUT_BYTES = 16 * 1024 * 1024`, `src/enrich/index.ts:31`, per-`grim`-call
`maxBuffer`). Reused two days later in `de0aac1` (2026-08-28
`feat(enrich): publish a checkpoint, and seed the sidecars from it`): the
checkpoint seed read (`src/enrich/checkpoint.ts:171`, `loadCheckpoint`) and the
write-side warning (`:150-157`: published anyway, "will be published but not
read back") use it. The doc comment in `http.ts` states the intent: "the largest
thing this toolchain holds in memory is one number rather than three". **No
note ties 16 MiB to a package count**; the ~700-package figure is only the
discussion's extrapolation (`index-scale-rollout.md`, Threads). The checkpoint
is self-contained (each `data.json` verbatim + payloads base64); the commit body
records the rejected alternative of splitting bookkeeping to `all.json`
("~5 % of the bytes ... a second fetch and three silent-failure couplings").
Failure of the seed is non-fatal by design ("only costs bandwidth", contrasted
with the ratings seed where an empty seed wipes ratings).

**Why checkpoint exists:** scaffolded CI never persists `enrich/<ns>/<name>/data.json`
so every deploy re-downloaded every README/changelog/logo/payload and re-dated
every artifact with no `created` (`updated` churn). `docs/reference/stats-sidecar.md`
(~L222-250): "`describe` still runs once per package and is never skipped ... The
checkpoint saves the downloads, not the probe."

**Why enrich is sequential:** `src/enrich/index.ts:340-343` on `main`
(19fe2dc):

> `// ponytail: sequential, like the Python it replaces. Each package is a couple of round trips and the digest probe skips almost all of them; batch with a worker pool if an index ever grows into the hundreds.`

Origin `b703767` (2026-07-28, `feat(enrich): read package READMEs, logos and
versions from the registry`), a port of a Python script. The 16 MiB / sequential
pair were designed for "hundreds", per that comment.

**Unmerged work that changes this (local branch + worktree, not on `main`):**
`feat/marketplace-enrich-landing` (worktree
`~/dev/grimoire-duo/.agents/worktrees/indexer-mkt`, tip `b1e3d11`, 2026-09-29,
11 commits ahead of `main`; remote-only copy exists too). Relevant commits:
`32665a3` `feat(enrich): carry unchanged packages forward on a digest probe`;
`db31a0a`/`b5d2a33` companion-digest probe fixes; `808550a` slice ordered by
bytes (locale-independent); `77e02fa` `--concurrency` digits-only validation.
Its `src/enrich/index.ts` header: probe `fetch --digest-only` first; unchanged
packages carried forward verbatim (no `describe`, no download); a **rotating
slice (the ceil(N/7) described longest ago)** re-described every run so edits
the digests cannot see land "within about a week"; "bounded pool" with
`DEFAULT_CONCURRENCY = 8`; a per-package failure keeps the existing sidecar
("stale beats empty"), only a majority failure reports an outage. Not merged, so
the sequential `main` behaviour is current.

**Build-time / scale work that did land (renderer side):**
`1e5247d` (2026-08-31) `feat(dev): --bulk, to look at the catalog at corporate
size` (clone fixture up to a count; Lighthouse "bulk" gate defaults to **250**
packages, `scripts/quality-site.mjs:26`); `2a77c15` `perf(renderer): build a
viewport of the catalog, not all of it` (measured at **500 packages**/442-result
set: stored-list-view paint 1963 ms to 387 ms, landing page 3.42 MB to 795 KB;
island renders a growing slice; `sitemap.xml`, `robots.txt` and a `<noscript>`
full list added so SEO/no-JS are not cut to the first slice); `f3d9ff5` island
props trimmed (323 KB to 201 KB attribute at 250 packages). **No commit, doc or
`.agents` note in the indexer discusses build time of `enrich`/`build` at 1k+,
sharding, or gzip.** `.agents/` there holds only `plans/theme-overlay-findings.md`.

**Frozen outputs** (`docs/reference/output-layout.md` "Frozen", mirrored in
`docs/index.md` "What is frozen"; added with the docs site in `88091f3`,
2026-08-30 `docs: publish a documentation site, and start a changelog`):
only **`/p/<namespace>/<name>/`** (grim resolves against it, VS Code deep link,
search engines) and **`/all.json`** plus the per-record `schema` field. "Everything
else may still move"; `/enrich.json` is explicitly "this package's own
checkpoint, read back by the next run's `enrich --seed`, and **not** a read
contract". `stats.json` is a separate documented contract
(`docs/reference/stats-sidecar.md`, grimoire `stability.md:40`: path and document
shape frozen under its own `schema_version`; presence not promised). Layout
tree also lists `index/<host>/<ns>/<pkg>/metadata.json` as "the pointer tree,
served as data" (not marked frozen on the indexer side; frozen on the grimoire
side as the git transport). `all.json` is written by `compileIndex` and ships
`contentDigest`, strips `descDigest` (`de0aac1` body). `public/` is layered so
`all.json` survives Astro emptying `dist/`.

### 4. Prior attempts, reversals, abandonments

- `git log --all -S` over `src`, `Cargo.toml`, `test`, `docs` in **grimoire**
  for `If-None-Match`, `ETag`, `Accept-Encoding`, `gzip`, `INDEX_BODY_LIMIT`,
  `IndexValidator`, `sharded`: **no hits** (a `segment` hit list is unrelated
  hook/export work). In **grimoire-indexer** for `gzip`, `ETag`, `If-None-Match`,
  `shard`: **no hits**. No attempt was ever committed, so none was reverted.
- The only "abandoned" artefacts are the parked ADR/plan above, and the
  *within-ADR* reversals: O3 `Freshness` policy withdrawn (2026-08-12); OCI HEAD
  digest gate and `ls-remote` probe dropped from the plan (owner ruling,
  2026-08-12); git hardening first split off, then pulled back in.
- Rejected-and-recorded alternatives relevant here: build-time prefilter of the
  500 window (`adr_registry_browse_filters.md` D6 alternatives); a config knob for
  TTLs ("rejected as YAGNI, and named here so it is not re-proposed", freshness
  ADR D8); `http-cache-reqwest 1.0.0-alpha.7` (freshness ADR Industry Context,
  point 2); splitting the enrich checkpoint bookkeeping onto `all.json`
  (`de0aac1`); Option 2 "forge-as-database at scale" for ratings ("right
  architecture at scale and the wrong one now", `adr_artifact_ratings.md`).
- Truncation history: `91b39070` made the cap *visible* (warning), never
  removed or raised it; `5b86337c` (2026-08-31) added per-source load status to
  the search JSON envelope. `adr_registry_browse_filters.md` records a
  *lost test*: `filter_never_rescues_a_build_truncated_group_c008` was deleted by
  `f790273` on a review branch (`handover_registry_set_review.md:290-295`);
  whether it was restored on `main` was not checked.
- Name overlap to avoid: `adr_projection_over_index.md` is about the TUI's
  `rows`/`filtered` index spaces, not the package index. Irrelevant here.

## negative:

- No ADR, plan, research, retro entry or memory note records why the
  freshness ADR stalled beyond "owner has not accepted"; no reason beyond
  ordering of other work can be inferred from the record.
- No document in either repo gives a stated expected `all.json` size, an
  entry-count ceiling for index sources, a 500-cap rationale for index
  sources, gzip, or a sharding plan. The 500 cap on index sources is a copy
  from the OCI `_catalog` walk with no recorded reason (`56d72a92`).
- No indexer note on enrich/build time at 1k+; the 16 MiB seed limit has no
  recorded package-count derivation.
- The live-header spot-check for `index.grimoire.rs` (ADR Open question 1) is
  not recorded as done.
- I did not run the build or tests; all claims are from reading source and `git log`.

## leads:

- `~/dev/index/.claude/artifacts/adr_enumeration_index.md` (30 KB, exists):
  the OCX sibling index's digest root-map layout, cited by the discussion
  as candidate (b); outside both repos, not read here.
- Indexer branch `feat/marketplace-enrich-landing` (worktree
  `~/dev/grimoire-duo/.agents/worktrees/indexer-mkt`): worker pool
  (`DEFAULT_CONCURRENCY = 8`), digest-probe carry-forward and N/7 rotating
  slice already exist unmerged; check its merge plan before designing around
  "enrich is sequential".
- `535b27c9` (ratings sidecar) is the template for stale carry-forward and
  fetch-status branching (404/410 = observation, 5xx/transport = no
  observation) that a failed-index-rebuild fallback could mirror; also where
  the wedge fix landed.
- `src/catalog/registry_catalog.rs:831-834` (cap on `build_from_index`),
  `src/catalog/index_source.rs:34,585-605` (30 s total timeout, unbounded
  body), `Cargo.toml:58` (no reqwest `gzip`) are the exact code sites.
- Untracked, written today for this discussion:
  `.agents/research/research_index_findability_change_rate.md`,
  `.agents/research/research_internal_catalog_success_at_scale.md` (not
  summarised here).
- `docs/src/content/docs/stability.md:40` and `package-index.md:151-157` are
  the grimoire-side statement of what is frozen for any layout change.
