# Research: Shell activation — existing sources for an update-check cache

## Metadata

**Date:** 2026-11-10
**Domain:** cli
**Triggered by:** /hex-discuss shell-activation
**Expires:** 2027-05-10

Scope: read-only recon. Grimoire = `/home/mherwig/.herdr/worktrees/grimoire/uno` (grim 0.15.1, branch `uno`); indexer = `/home/mherwig/dev/grimoire-rs/indexer`. Paths below are relative to the repo named, grimoire unless prefixed `indexer:`. No recommendation is made. Complements `.agents/research/research_shell_activation_recon.md` (Q3 there: `status --check` is live, uncached, untimestamped; catalog TTL 3600; no "outdated" cache; read it first, not repeated here).

Method note: timings use the already-built `target/release/grim` (0.15.1) against the live `~/.grimoire` (`GRIM_HOME` unset). Nothing was built, no network was used. I read, but did not modify, `~/.grimoire/tags/**` and `~/.grimoire/catalog/**`.

## Q1. OCI access layer cache

**What exists.** `CachedAccess` wraps the registry client with a persistent `TagCache` plus the content-addressed `BlobStore` (`src/oci/access/cached_access.rs:4-9`, `:38-43`). Wired at `src/context.rs:385`.

**What is stored.**
- Tag pointers: one file per repository, `$GRIM_HOME/tags/<registry>/<repo with / -> _>/tags.json` (`src/oci/tag_cache.rs:57-63`, `src/store/paths.rs:64-66`). Content is `{version:1, registry, repository, tags:{<tag>: <manifest digest>}}` (`tag_cache.rs:34-43`). Live example: `~/.grimoire/tags/ghcr.io/michael-herwig_arcana_hex-core/tags.json` holds `"latest": "sha256:64bedd52..."`.
- Manifests and blobs: content-addressed in `$GRIM_HOME/blobs/sha256/`, keyed by digest (`cached_access.rs:97-140`, `:163-203`). A manifest is read by pinned digest, so this answers "what is in digest D", never "what does a tag point at".
- Tag **names**: `list_tags` writes through `put_tags`, which only prunes digests for tags no longer listed (`cached_access.rs:205-216`, `tag_cache.rs:136-157`); it never adds a digest.

**Timestamps / TTL.** None. `TagCacheFile` has no timestamp field (`tag_cache.rs:35-43`); there is no TTL logic in `TagCache` or `CachedAccess`. The only time signal is the file's mtime, and `put` rewrites the whole repository file on every insert (`tag_cache.rs:112-125`, `:159-164`), so mtime means "last `Resolve` that touched any tag of this repository", not per tag. Live evidence: three files dated 2026-11-04, two dated 2026-11-10 19:49-19:50.

**Who writes it.** Only `Online` + `Operation::Resolve` (`cached_access.rs:84-90`). `Resolve` callers: the lock/update resolver (`src/resolve/resolver.rs:1322`), `fetch` (`src/fetch.rs:337,807,1042`), release (`src/command/release.rs:763`). `Operation::Query` never writes it (`cached_access.rs:21-24`, `src/oci/access.rs:73-85`). `grim status --check` and the TUI checker use `Query` (`src/catalog/update_availability.rs:44-47`, `:57-62`), so they leave it untouched.

**Who reads it.** Only `Offline`. `Online` never reads the cached tag pointer, by design: "a floating tag always reflects the registry" (`cached_access.rs:21-25`, `:79-83`; `access.rs:53-60`). Under `GRIM_OFFLINE` / `--offline` the routing is: tag cache hit -> digest; miss -> `Resolve` errors `OfflineMiss`, `Query` returns `Ok(None)` (`cached_access.rs:69-78`). A digest-addressed id returns itself with no I/O (`:64-67`). Manifests/blobs offline: store only, miss is `OfflineMiss` (`:113-118`, `:178-180`). `list_catalog` offline returns empty, there is no persistent `_catalog` cache in this layer (`:218-224`).

**Could it answer "tag -> digest as of T" today?** Partly, and only as "the digest the last `Resolve` on this machine saw", with no T and no bound on age:
- The write happens in the same resolve that produces the lock pin, so for a workspace whose lock was just written the cached digest equals the pin. It differs from the pin only if another workspace or command later resolved the same `registry/repo:tag` to a newer digest (the file is machine-wide, shared across all projects).
- There is no scheduled refresh; an entry is exactly as old as the last lock/update/add/fetch that touched that repo, and can be arbitrarily old (no eviction exists; grep for expiry/prune in `tag_cache.rs` finds only the `retain` in `put_tags`).
- The file format is `deny_unknown_fields` (`tag_cache.rs:36`), so adding a timestamp field is not additive for an older grim reading a shared home: an older binary would reject the file (a read failure surfaces as an I/O error, `tag_cache.rs:67-75`). Schema is `serde_repr` versioned (`:27-32`), so a V2 would be the route; unverified whether an older grim treats a V2 file as fatal or cold (`read_file` returns `InvalidData`, which `CachedAccess` maps to an access I/O error, `cached_access.rs:71`).
- Not documented as a layout surface: grep of `docs/src/content/docs/configuration.md` "Data layout" and `.claude/rules/subsystem-file-structure.md` finds no `tags/` entry. Looked for it, did not find it, so Principle 9 status is unclear (undocumented, but on disk in every install).

## Q2. Catalog sync and refresh as shipped

**Load path.** `load_catalogs` fans out one load per configured source (`src/catalog/catalog_service.rs:384-433`). Index sources go through `Catalog::load_or_refresh_index_coordinated` (`src/catalog/registry_catalog.rs:786-830`), `oci` sources through `load_or_refresh_coordinated`.

**TTL.** `CATALOG_TTL_SECONDS = 3600` (`registry_catalog.rs:41`). Freshness = the cache's `built_at`, or a newer `checked_at` in the sync sidecar (`registry_catalog.rs:506-523`). A `truncated` cache is stale however young (`:838-841`, `:859`). Within TTL: fast path serves with no lock and no request (`:870-874`).

**Offline.** Serves whatever is cached (any scope), never locks, never touches the network; cold cache = empty (`registry_catalog.rs:864-868`).

**Sidecar.** `$GRIM_HOME/catalog/<h16>.sync.json` (`src/catalog/sync_state.rs:71-92`; layout doc `.claude/rules/subsystem-file-structure.md`, section Catalog Cache; `docs/src/content/docs/configuration.md` Data layout). Holds `checked_at`, adopted root digest + generation, flat/stats digests, `failures`, `last_failure_at`, `last_error`, `retry_after`. Binds to the cache file by `catalog_sha256`; mismatch = ignored whole, one fresh sync. Live example (`~/.grimoire/catalog/8630d4061b4b8621.sync.json`): `layout: anchored`, `entries: 21`, `checked_at: 2026-11-10T18:49:50Z`, `fallbacks: 0`, stats applied == advertised.

**Backoff.** 60 s doubling, ceiling 3600 s, 20% jitter, `Retry-After` can lengthen (`sync_state.rs:37-43`; `docs/src/content/docs/package-index.md` "Failures and Backoff", lines 192-210). While deferred: no request, serve cache, source reports `ok: false`.

**Refresh modes.** `Refresh::{No, Yes, StatsForced}` (`sync_state.rs:23-33`): `No` = past TTL only; `Yes` bypasses TTL and backoff; `StatsForced` bypasses TTL only, keeps backoff (`registry_catalog.rs:852-855`).

**Locking.** Per-source `AdvisoryFileLock::try_acquire` on the cache path; a peer holding it means serve stale and move on; readers never block (`registry_catalog.rs:882-896`; `src/lock/file_lock.rs:3-9` per recon).

**No-change revalidation cost.** Past TTL, an index source does: one `GET <index>/c/root.json` (~0.5 KB, capped at 64 KiB, gzip accepted; `src/catalog/index_root.rs:14`, ADR `.agents/adr/adr_catalog_at_scale.md:286-327`), 10 s probe timeout (`src/catalog/index_sync.rs:39`). If the root's flat digest equals the synced one, the flat blob is not fetched and the outcome is `Restamp`: only the sidecar is rewritten, the `CatalogFile` is not (`index_sync.rs:474-512`, `:58-60`; `.claude/rules/subsystem-file-structure.md`: "A quiet refresh ... writes the sidecar only"). The stats blob costs one more GET only when the root advertises a stats digest not yet applied (`index_sync.rs:534-`; `sync_state.rs:StatsState`). **There is no conditional request**: grim sends no `If-None-Match`/`If-Modified-Since`, stores no validator, and a 304 classifies as a miss (`.agents/adr/adr_catalog_at_scale.md:447-449`, "Cut by owner decision"). So a no-change revalidation = 1 round trip, ~0.5 KB body, once per TTL window per source. If the root probe fails with no adopted root, it falls back to `/all.json` (full body) (`index_sync.rs:332-400`, `:401-456`).
- An `oci` source's revalidation is a registry walk (`_catalog` plus per-repository tag list and manifest read), reusing rows whose tag and digest are unchanged (`registry_catalog.rs:1195-1235`, `:1400-1410`; `commands.md` search row in `.claude/rules/subsystem-cli-commands.md`). Cost scales with repository count; not measured.

**Which commands trigger it.** `grim search` (`src/command/search.rs:176`), `grim status --check` (`src/command/status.rs:701-709`, `Complete` scope, online only), `grim rate` row resolution (`src/command/rate.rs:743-751`), the TUI at startup / `r` / scope toggle (`src/tui/app.rs:1341`). `grim status` without `--check` never loads the catalog (recon Q2: "no network and no flock"). Decided-but-proposed items (async TUI load, 1 s `search` deadline) live in `.agents/adr/adr_catalog_freshness_revalidation.md` ("Accepted in part"); I did not verify each against code.

## Q3. Index content

### Legacy `/all.json` and the newer layout: shape

`all.json` is an array of package pointers. grim's reader, `IndexPackage`, consumes: `schema`, `name`, `namespace`, `kind`, `ref` (= `registry/repository`, **no tag**), `description`, `repository`, `keywords`, `summary`, `deprecated`, `replaced_by`, `license`, `created`; unknown fields are tolerated (`src/catalog/index_package.rs:15-60`). The projection hard-codes `latest_tag: None, version: None`, with the comment "Phone-book contract: no version data in the index; tags are resolved live from the registry at install time" (`index_package.rs:112-117`; test `src/catalog/index_source.rs:635`).

The cached entry (`CatalogEntry`, `registry_catalog.rs:233-325`) has `latest_tag`, `version`, and `oci.digest`, but for an index source all three are empty. Confirmed on disk: the live cache `~/.grimoire/catalog/8630d4061b4b8621.json` (21 entries, source `https://index.grimoire.rs`) has keys only `created description fetched_at index_path keywords kind oci rating registry repository repository_url summary`, and 0 of 21 entries carry `oci.digest`; none carry `latest_tag` or `version`.

The indexer's records are richer than what grim reads. `all.json` merges the enrichment sidecar into each record (`indexer:src/data/index.ts:199-205`; only `descDigest` and `describedAt` are stripped). The sidecar carries `version`, `tags` (names only, from `describe`, bound 128 x 64 B), `contentDigest` (`indexer:src/enrich/index.ts:198`, `:520`; `indexer:src/enrich/sidecar.ts:48`; bounds `indexer:src/emit/records.ts` LISTS/TEXT). `contentDigest` is the digest from `grim fetch <ref> --digest-only` on the pointer's tagless ref (`indexer:src/enrich/index.ts:467-479`), which grim resolves as `latest` (`tag_cache.rs:100`, `tag_or_latest`). **Unverified:** whether `contentDigest` survives into the published `all.json`/flat blob (it is not in the documented allow-list; it would ride the 1 KiB unlisted-key budget, `indexer:src/emit/records.ts`), and no live `all.json` was fetched. Grim ignores it regardless. Nowhere in the indexer is there a per-tag digest: searched `indexer:src` and `indexer:docs/reference/` for a per-tag digest map and found none.

Newer layout, **shipped in grim code**: `c/root.json` probe, adoption of the content-addressed gzip flat blob `c/f/sha256/<hex>.json.gz` (same bytes as `all.json`), stats blob, sync sidecar, fallback to `/all.json` (`index_sync.rs:332-512`, `index_root.rs`, sidecar `layout: anchored` live). The flat blob has the **same record shape** as `all.json`, so it adds no tag/digest data (ADR D2: "inflated bytes are the frozen formats", `.agents/adr/adr_catalog_at_scale.md:286-327`). The ADR's own status line says Proposed (`adr_catalog_at_scale.md:5`) although the above is implemented.

**Designed or indexer-private, not consumed by grim:** `c/p/` plane index and `c/s/` 256 hash-bucket shards are "pipeline-private, read by the indexer only" (`adr_catalog_at_scale.md:286-327`; `indexer:docs/reference/output-layout.md:133-`); grim's `parse_root` leaves `planes` untyped and unread (`index_root.rs:30-41`, `:68-70`). The hashed record tree (`planes.catalog`, `c/r/`, delta sync, 256 MiB body) is **Deferred** (`.agents/adr/adr_catalog_tree_sync.md`, per the ADR row in `.claude/rules/arch-principles.md`); no code in `src/catalog/` mentions shards or `c/r/` (grep -i 'shard|c/r/' returns none outside `index_root.rs` comments). `c/q/` is site-only.

### Can the cached catalog answer `repo:tag -> digest`?

| Case | Index source (what is cached today) | `oci` `_catalog` source |
|---|---|---|
| (i) exact version tag `:1.2.3` | No. No tag->digest map; `tags` (names) is not even read by grim. | No. One representative tag only (`latest` else highest semver, `registry_catalog.rs:1542-1560`) and its digest in `oci.digest` (`:1458-1460`); answers only if the declared tag happens to be that representative tag. |
| (ii) floating tag `:1`, `:0.12`, `:latest` | No. | Only `:latest`, and only when `latest_tag == "latest"`. `:1`/`:0.12` never. |
| (iii) digest pin `repo@sha256:...` | Trivially, without any cache: a digest id resolves to itself (`cached_access.rs:64-67`, `tag_cache.rs:96-99`). | Same. |

The one candidate that exists is the `oci.digest` of the representative tag, and for index sources it is absent (live: 0/21). So the cached catalog **cannot** answer `repo:tag -> digest` for (i) or (ii) from anything grim reads today. What an index producer could add is a separate matter (see closing table).

## Q4. Lookup cost

**Structure.** One cache file per source, parsed whole with `serde_json::from_slice` into `BTreeMap<String, CatalogEntry>` keyed `registry/repository` (`registry_catalog.rs:398-412`, `:539-570`; live file keys such as `ghcr.io/grimoire-rs/bundles/grim-essentials`). No shard or hash-prefix lookup exists in shipped code (grep for shard/planes in `src/catalog/*.rs`: only `index_root.rs` comments). After the one-time parse a lookup is an in-memory `BTreeMap` `get` (O(log n), well under a microsecond for thousands of keys). There is no `get_entry(ref)` helper; `entries.get(...)` appears only in tests and in `index_sync.rs:292` (stats carry), so a consumer would add a thin accessor. A reference must be matched against every configured source's file (one per locator hash), since an entry lives in whichever source listed it.

**Measured** (release binary, this worktree, `~/.grimoire` with one 27.3 KB, 21-entry cache; 10 runs each, median / min):

| Command | Median | Min |
|---|---|---|
| `grim --version` | 2.7 ms | 2.5 ms |
| `grim --offline status` (no catalog read) | 9.5 ms | 9.2 ms |
| `grim --offline search hex` | 13.2 ms | 12.9 ms |
| `grim --offline search` (all rows) | 14.2 ms | 13.7 ms |
| `grim --offline search --format json hex` | 13.3 ms | 12.9 ms |

`search` also loads config and resolves registries and badges (lock/state reads), so the catalog parse itself is a small part of 13 ms; I did not isolate it. Floor is process start plus the Tokio runtime (recon Q7).

**Estimate for scale (not measured).** ~1.3 KB per cached entry (27.3 KB / 21) puts 20,000 entries (the ADR design point) at roughly 26 MB; the ADR itself cites >= 41 MB serialized at 20,000 (`adr_catalog_at_scale.md:447-460`, D8 text). A full `serde_json` parse into the map at that size is plausibly tens to low hundreds of milliseconds per run, paid once per process regardless of N. N reference lookups after the parse add negligible time. The per-run cost is dominated by file read + parse, so it does not scale with how many references the caller checks, only with index size. Today's public index is 21 entries, so the shipped cost is the table above.

## Q5. Freshness and agreement

**Index lag behind the registry (index-side).** The index is a build artifact, not a live view:
- Pointer content comes from the index repository (a package is announced by a PR/pointer; `src/catalog/index_announce.rs`).
- Registry-derived data comes from `enrich`, which re-checks each of 256 buckets once per `sync.revalidate` window, default 1 h, configurable 15 m to 7 d, buckets staggered; full pass at most every 7 windows (`indexer:docs/reference/cli.md:208-224`; `indexer:docs/reference/config-schema.md:231`, `:293-299`: "a release that reaches the site up to one window later").
- `enrich` runs inside the Pages build (`indexer:templates/ci/github-pages.yml:8-11`), which triggers on push to the default branch, manual dispatch, and an hourly `23 * * * *` schedule **only when stats are configured** (`indexer:src/ci.ts:327`, `:479`). An index without stats has no cron, so its registry data moves only when someone pushes.
- Then grim's own TTL adds up to 3600 s (`registry_catalog.rs:41`) because there is no conditional request and no short floor (`adr_catalog_at_scale.md:447-449`).
- Net: best case minutes, typical order of 1-2 h, unbounded for an index with no schedule. No document states a publish-to-index SLA; I looked for one (grep of `indexer:docs/ops/known-limitations.md`, `index-vs-site.md`) and found only the per-window statements above.

**Stated reason for "always live" (`commands.md:1496-1500`).** "The resolution is always live, so a tag that moved surfaces even when the cached catalog row is stale. That makes it the check for a pin `grim lock` kept behind a moved tag, or one that was hand-edited." Code-side rationale: `src/catalog/update_availability.rs:14-20` (the question is "would `grim update` move this pin", so it resolves the declared reference through the same `resolve_digest` the resolver uses, `:37-46`, so "the two can never disagree") and `:50-56` ("Freshness against a stale cached catalog tag (issue #21) is preserved regardless"). I did not find an explicit statement of a latency or rate-limit reason, nor one forbidding a cache beyond "always live".

**Would an index-derived answer disagree with a live resolve?** Yes, in a predictable direction, when the index data lags: the live registry is ahead, so an index-derived digest would be **older** than live, producing false "up to date" (a missed update) right after a publish. The reverse (index ahead of registry) occurs only if a tag was moved back or deleted after the index read it, giving a false "update available" until the next enrich. The doc's point on tag moves applies equally: a moved floating tag is invisible until the next enrich window. Also a pure `repo`-level answer (latest version) would disagree with a declared narrower reference: `update_availability.rs:50-56` records that an earlier repo-head check badged a `:0.12.0` pin outdated forever. That is a disagreement by question, not by lag.

## Q6. Coverage

How the code separates the cases:
- Lock source kind: `LockedSource::{Registry(PinnedIdentifier), Path{..}}` (`src/lock/locked_source.rs:41-54`); bundles `LockedBundleSource::{Registry, Path}` (`src/lock/locked_bundle.rs:35-50`). `status` schedules a re-resolve only when a registry pin and a declared identifier exist; "path/dev rows carry no pin" (`src/command/status.rs:465-485`; bundle path-sourced skipped `:339-358`).
- Browse source kind: `SourceKind::{Registry, IndexHttp, IndexGit}` (`src/config/registry_resolve.rs:28-50`).

Per category, what a catalog-derived answer could cover:
- **Plain OCI `[[registries]]` without an index:** the catalog is built from the registry `_catalog` walk, which GHCR, Docker Hub and GitLab SaaS gate or omit (`registry_catalog.rs:47` `CATALOG_GATED_REGISTRIES`; `.claude/rules/product-context.md` "Discovery: The Index"). Where `_catalog` works, rows carry one representative tag + digest; where it does not, no rows. Compatibility doc: `registry_catalog.rs:52` `REGISTRY_COMPAT_DOCS_URL`.
- **Private registries:** a private repository the index does not list is absent; an `oci` walk skips what it cannot read ("Private, transient: never a hard catalog failure, and no digest is recorded", `registry_catalog.rs:1433-1436`). `Resolve` through `CachedAccess` works because it uses credentials (`grim login`); the catalog has no per-row auth record.
- **Path sources / dev installs:** no registry pin, nothing to compare (`locked_source.rs:47-53`, `status.rs:465-485`).
- **Bundle members:** their lock rows carry a registry pin plus bundle provenance; `status --check` re-resolves each member by the id its bundle listed, and the bundle row against the `[[bundle]]` snapshot (`commands.md` ~1445-1465; `status.rs:339-358`, `:642`). A catalog entry exists only per repository the index lists, so members whose repositories are not in the index have no catalog row.
- **Cross-registry dependencies / transitive rows:** lock keys include the registry host, and an index serves rows from many hosts (`catalog_service.rs` CatalogGroup docs); coverage depends only on whether that `registry/repository` is listed by some configured source. Dependency rows resolve through recorded edges whose tag may differ from the row's own pin (`.claude/rules/subsystem-cli-commands.md` `status` row, `dependency_reference`); a catalog keyed by repository carries no per-edge tag.
- **Browse filters:** `Complete` scope is never filtered, `Browse` is (`catalog_service.rs` `CatalogScope`; `commands.md:1470-1475`).

Nothing in code distinguishes "indexed" from "not indexed" for a locked row today; the only existing per-row signal is the catalog join by `registry/repository` in `status --check` (`status.rs:701-709`, `apply_catalog_load`), which feeds `deprecated`/`replaced_by`, not digests.

## Q7. How `search` and the TUI use catalog cache versus live resolves

- `grim search`: catalog cache only (`search.rs:176`); never resolves a tag; shows `latest_tag`/`version` where the source provided them (`search.rs:320-321`), empty for index sources.
- `grim status --check`: catalog load (`Complete`, only for `deprecated`/`replaced_by`) plus **separate** fresh per-row `Operation::Query` resolves (`status.rs:701-730`; `commands.md:1496-1500`). Catalog digests are not used for update availability.
- TUI: `catalog_service::load_catalog` at startup/refresh (`src/tui/app.rs:1341`) builds rows and install badges from lock+state (`catalog_service.rs:181-195`, `derive_badge`); outdated markers come from a background `UpdateChecker` that spawns bounded (8) `resolve_declared_digest` calls per installed row (`src/tui/update_check.rs:289-342`, `:35-40`), compares with the locked digest via `outdated_from_resolve`, and flips the row with `mark_outdated_if_installed` (`src/tui/state.rs:1182-1190`). Results are held in memory only; none persist. `spawn_catalog_refresh` is `dead_code` for the event loop (`update_check.rs:` `CheckMsg::CatalogReady` allow-attribute). Recon records the decision that a background refresh has no owner in a short-lived CLI (`plan_catalog_freshness_revalidation.md:226-230`).

## Q8. "Last checked" on disk

Found:
- `catalog/<h16>.sync.json` `checked_at`, `catalog_built_at`, `last_failure_at`, `retry_after` (`sync_state.rs:71-92`): per catalog **source**, not per artifact or reference.
- `catalog/<h16>.json` `built_at` and per-entry `fetched_at` (`registry_catalog.rs:325`, `:398-412`): when the listing/entry was fetched.
- `tags/.../tags.json`: no timestamp; file mtime only, per repository (`tag_cache.rs:34-43`).
- `grimoire.lock` `[metadata] generated_at` (live: `2026-11-04T11:46:41Z`): when the lock was written, per workspace, not a check.
- `.grimoire/state.json` records `pinned` as `repo:tag@sha256:...` plus output content hashes, no timestamps (live file read).
- `~/.grimoire/state/` is empty here; `$GRIM_HOME/state/votes.json` and consent records are unrelated to checks.
- Index/stats `generated_at`, `as_of` ride the index, not a local per-artifact record.

Not found: any per-artifact, per-reference or per-workspace "last update check" record; any cached result of `status --check`; any "outdated" file; grep for `last_check`/`last_checked` in `src` finds only catalog sync code.

## Closing table: candidate sources

| | (a) New machine-wide per-reference cache (`registry/repo:tag` -> digest + checked-at) | (b) OCI access layer's existing tag cache | (c) Catalog / index cache (incl. hashed index layout) |
|---|---|---|---|
| **Can answer** | Exactly the question asked, for any declared reference including floating tags, if something fills it. | Exact `repo:tag -> digest` for any tag a `Resolve` has touched on this machine (`tag_cache.rs:96-125`); digest pins trivially. | Repository-level metadata and, for `oci` sources only, one representative tag + digest (`registry_catalog.rs:1458`, `:1542`). Digest pins trivially. |
| **Cannot answer** | Nothing until populated; cold for a new reference. | Staleness (no timestamp, no refresh); anything never resolved; "as of T". Cannot distinguish "unchanged" from "never rechecked". Online reads are deliberately disabled (`cached_access.rs:21-25`). | `repo:tag -> digest` for index sources (no data in the cache or the `all.json`/flat record grim reads); floating tags `:1`, `:0.12`; exact version tags; private or unlisted repos; path sources. |
| **Cost per lookup** | One small file read or in-memory map; per-reference live resolve is the fill cost (one manifest HEAD/GET per distinct reference, concurrency 8, `status.rs:72-74` per recon). | One small JSON file read per repository (`tag_cache.rs:75-85`); microseconds to sub-millisecond. | Whole-source parse once per process (27 KB file here; est. tens to low hundreds of ms at 20,000 entries, unmeasured), then in-memory map get. Network: 0 within TTL, else 1 GET of `c/root.json` (~0.5 KB) per source per hour, no conditional request. |
| **Staleness bound** | Whatever the design sets (checked-at it carries); only as fresh as the last live resolve that wrote it. | Unbounded; equals the last `Resolve` that touched the repo (lock/update/add/fetch/release); mtime is the only hint and covers the whole repository file. | Up to grim TTL 3600 s + indexer window (default 1 h, range 15 m to 7 d) + build cadence (hourly only if stats schedule configured, else on push), so typically 1-2 h, unbounded for an unscheduled index. |
| **What would have to be built** | New store, writer, schema, locking, a populate path, and a decision on who runs live resolves (the "no owner" decision, `plan_catalog_freshness_revalidation.md:226-230`). Nothing existing writes it. | A timestamp (new schema version; `deny_unknown_fields` blocks an additive field for older grims), a writer path from `Query`-class checks (today `Query` is forbidden to write, `cached_access.rs:21-24`), and a decision on ownership of the file's contract (undocumented). | Producer change: per-tag digests or a digest for the declared tag in `all.json` records (none exist; `contentDigest` is latest-only and unverified in the published file); grim `IndexPackage` + `CatalogEntry` fields (additive, tolerant of unknowns, `index_package.rs:15`); a lookup accessor; coverage for `oci`/private/unlisted repos remains a gap. |

## What I looked for and did not find

- Any timestamp, TTL or eviction in the tag cache (`tag_cache.rs`, `cached_access.rs`).
- Any user documentation of `$GRIM_HOME/tags/` (configuration.md Data layout, subsystem-file-structure.md).
- Any per-tag or per-reference digest in the index record shape, the `IndexPackage` reader, the live 21-entry cache, or anywhere in the indexer source/docs reviewed.
- Any shard or hash-prefix lookup in grim (`src/catalog/`).
- Any conditional-request support (cut by owner decision, `adr_catalog_at_scale.md:447-449`).
- Any local "last update check" record per artifact, reference or workspace.
- A published publish-to-index latency statement.
- A live fetch of `all.json` or `c/f/` blob (no network used), so `contentDigest` presence in the published file is unverified.
- Whether an older grim tolerates a V2 `tags.json` (read path suggests a hard I/O error; not tested).
