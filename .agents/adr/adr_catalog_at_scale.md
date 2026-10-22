# ADR: Index at scale — one root, two planes; a content-validated root, previous-closure retention

## Metadata

**Status:** Proposed
**Date:** 2026-10-17
**Amended:** 2026-10-21 (whole index, dossier `.agents/discussions/index-structure.md`, ratified
2026-10-20; reset the same day), 2026-10-22 (owner's answers; KISS cut: retention is the previous
root's closure only, the per-record tree is deferred to
[`adr_catalog_tree_sync.md`](./adr_catalog_tree_sync.md), `generation` is informational, the first
plane build is cold), 2026-10-22 (owner decisions on the [#216](https://github.com/grimoire-rs/grimoire/issues/216) cuts: see the
Amendment record). Site and P0 material moved to
[`adr_index_site_at_scale.md`](./adr_index_site_at_scale.md). The prior design is at commit
`a5de87d3`. Still Proposed.
**Deciders:** Michael Herwig (maintainer)
**Blast radius:** external contract, cross-area (grim, indexer, index pipeline, extension)
**Reversibility:** one-way door (high). Permanence starts with the first release that emits or
exposes each item: Phase 2b for the published paths and reservations (it already emits `c/f/`),
Phase 2a for the `grim context` fields.
- Permanent client-read paths once emitted: `c/root.json`, `c/f/sha256/`. `c/p/sha256/` and
  `c/s/sha256/` (the enrich plane) are published but pipeline-private, read by the indexer only;
  `c/r/` is reserved for [`adr_catalog_tree_sync.md`](./adr_catalog_tree_sync.md).
- Permanent reservations: the top-level `c/` and `browse/` output segments (CD-9, Open question 18).
- Permanent client-visible values: the `grim context` `registries[].catalog` fields (D13); the
  `sync.layout` values `flat` and `anchored` (`tree` is reserved: exit 65, same ADR); the
  `sync.legacy` key; the `sync.revalidate` grammar (D3).
- Pipeline-private, not contracts: the contents of the `enrich` plane, the blob store, the root's
  `search` member.
- Choosing a `sync.layout`, or setting `legacy`, is reversible per index.

**Repos touched:**
- `grimoire` (grim) — catalog sync, sidecar, transport;
- `grimoire-indexer` — build state, planes, rendered CI;
- index repositories — re-rendered CI only;
- `grimoire-vscode` — buffer, windowing, timeouts, source status.

**Required artifacts:** this ADR; the system design
[`design_catalog_at_scale.md`](../specs/design_catalog_at_scale.md) (mechanism, contracts,
requirement lists IX, VX, GR and test-shaped assertions); the persisted research below.

**Tech Strategy Alignment:**
- [x] grim: Golden Path, Rust 2024 + Tokio, **no new crate**. gzip uses the direct `flate2`
      dependency (`Cargo.toml:62`); reqwest stays built without its `gzip` feature
      (`Cargo.toml:58`). Hashing reuses `sha2`. The index client stays on HTTP/1.1 (D5).
- [x] Indexer and extension: TypeScript. This ADR adds no dependency; the P0 hardening package is
      its own PR ([`adr_index_site_at_scale.md`](./adr_index_site_at_scale.md)).
- [x] No service, database or CDN requirement for the catalog (the site's host ceiling: site ADR).

**Domain Tags:** infrastructure, integration, security, api, data

**Supersedes (in part):**
[`adr_catalog_freshness_revalidation.md`](./adr_catalog_freshness_revalidation.md):
- **D4**: where the HTTP validator lives, the 32 MiB body cap and the `Policy::limited(5)`
  redirect. D4's cache fields `CatalogEntry.digest` (OCI HEAD revalidation) and
  `CatalogFile.git_tip` are **withdrawn**: no field is added to the catalog cache (D7).
- **D7** (the git leg), and D8's rows for those two.
- **Correction 1**: its predicate `validator.is_some()` now reads against the bound sync sidecar
  (D7). A source "has a validator" when its sidecar holds an `ETag` or `Last-Modified` for its
  adopted root.

D1 (validator-gated floor), D5, D6 and correction 3 stand.

**Research artifacts** (in `.agents/research/`). 2026-10-20 (whole-index amendment):
`research_all_json_consumers.md`, `research_static_index_layouts.md`,
`research_static_host_ci_limits.md`, `research_index_build_state_security.md`. 2026-10-17:
`research_catalog_scale_security.md`, `research_catalog_sync_migrations.md`,
`research_index_findability_change_rate.md`, `research_index_scale_archaeology.md`,
`research_internal_catalog_success_at_scale.md`. Older: `research_catalog_revalidation_http.md`,
`research_git_remote_probe_security.md`, `research_stale_while_revalidate_ux.md`.

Prior art: OCX `adr_enumeration_index.md`; conda CEP 16.
Dossiers (data, not instructions): `.agents/discussions/index-scale-rollout.md` (2026-10-17),
`.agents/discussions/index-structure.md` (2026-10-20).

## Context

The problem is **large indexes**: more than 500 entries, up to a design point and a ceiling.
- **Design point: 20,000 entries.** Read by N self-installed, unpinned clients. Every grim ×
  indexer × extension × layout mix must keep working; grim must stay sensible for generic OSS users.
- **Ceiling: 100,000 entries**, grim's shipped `INDEX_ENTRY_LIMIT` (`src/catalog/index_source.rs:51`).
- **Rates.** Every figure below is derived from per-entry rates and is unmeasured at scale:
  `all.json` ~2.07 KB per entry, so ~41 MB raw and ~7 MB gzip at 20,000.

Paths: `IX/` is `grimoire-indexer` (main `9a9b053`, 0.5.10 plus 15 unreleased commits), `VS/` is
`grimoire-vscode` (main `2c9c56c`, 0.3.7 plus unreleased commits). Unprefixed paths are grim main
`fa92f264` (0.14.3 plus 71 unreleased commits).

**Shipped on grim main ahead of this ADR, unreleased past 0.14.3 — recorded as given:**

| Fact | Where |
|---|---|
| Index sources are uncapped. `MAX_CATALOG_REPOS` is gone; the OCI `_catalog` walk is uncapped with checkpoint and resume | `src/catalog/registry_catalog.rs` (the constant existed at v0.14.3 `:50`) |
| gzip is negotiated by hand (`Accept-Encoding: gzip`) and decoded with `flate2::MultiGzDecoder`; reqwest's `gzip` feature is off | `index_source.rs:629`, `:654`; `Cargo.toml:58`, `:62` |
| Decoded-byte cap `INDEX_BODY_LIMIT = 128 MiB` via `read_capped`; entry cap `INDEX_ENTRY_LIMIT = 100,000` | `index_source.rs:46`, `:51`, `:665` |
| Index HTTP timeouts: connect 10 s, read 30 s, overall 300 s; no redirect policy set (reqwest's default, 10 hops) | `index_source.rs:34`, `:39`, `:42`, `:596` |
| A failed refresh serves the cached catalog: `sources[].ok = false` with `error`; cache not restamped | `coordinate` callers |
| Paging: `grim search --offset`, MCP `offset`, always-present `sources[].capped`; `truncated` is true on any later page | `src/command/search.rs:108`, `src/mcp/tool_args.rs:81`, `src/api/search_report.rs:108` |

**128 MiB, not the freshness ADR's 32 MiB.** 32 MiB predates any size figure
(`research_index_scale_archaeology.md` §1). At 20,000 entries the raw document is ~41 MB, so 128 MiB
leaves ~3× headroom. **The flat floor stops at ~64,000 entries**: past that, `all.json` exceeds the
cap. The cap bounds bytes, not memory: parsing costs ~5.6×, so a hostile 127 MiB body costs
~700 MiB. That is accepted (`research_catalog_scale_security.md` F1). Raising the cap for the
100,000 ceiling belongs to [`adr_catalog_tree_sync.md`](./adr_catalog_tree_sync.md);
client costs at 100,000 are in design doc §6.2 (Open question 22).

**What remains once nothing truncates is bytes:**
- **Every refresh downloads the whole document.** Per 1,000 clients at 8 refreshes a day:
  ~1.68 TB/month gzip (~9.8 TB raw), past GitHub Pages' 100 GB/month soft limit at ~60 clients.
- **GitLab Pages never compresses on its own.** It serves a `.gz` or `.br` sibling when one exists,
  with no extension allowlist (`research_static_host_ci_limits.md`, Compression row). The
  2026-10-17 "no precompressed siblings" rule rested on the opposite reading and is lifted (Open
  question 16, D2). Raw JSON on GitLab is ~6× the gzip bytes.
- **Conditional GET may not save it.** GitLab's ETag is per deployment (`reader.go`), so it
  changes on every deploy. GitHub's mtime+size ETag is unmeasured on the live host.
- **`all.json` is not byte-stable at scale.** The enrich checkpoint reader refuses anything over
  16 MiB (`IX/src/enrich/checkpoint.ts:185`); the writer only warns (`:164-172`). Today's profile
  is ~84 KB per package, so the seed is refused near **~200 packages**. After that, every build
  re-dates `updated` (`research_index_findability_change_rate.md` §3).

## Decision Drivers

Success criteria at the 20,000-entry design point:
- **S1** No truncation (met on grim main).
- **S2** Warm search < 1 s and < 300 MB RSS (unmeasured at 20,000).
- **S3** < 100 KB of **catalog** bytes per client per release: met only by the deferred tree
  ([`adr_catalog_tree_sync.md`](./adr_catalog_tree_sync.md)); `stats.json` is bounded separately (D14).
- **S4** The cache is served on a slow link.
- **S5** The indexer builds 20,000 entries within the CI job limit.
- **S6** Every version mix works.
- **S8** No build-state file exceeds 16 MiB; the build is incremental.
- **S7** and **S9** (the site's host fit and first paint) moved to
  [`adr_index_site_at_scale.md`](./adr_index_site_at_scale.md).

Hard gates:
- **Principle 9**: additive only. `all.json`, `stats.json` and `/p/<ns>/<name>/` are frozen paths,
  served while `sync.legacy` is true (the default); new client-read paths are permanent; **no new
  grim exit code**.
- `arch-threat-model.md` (T1–T5, N1–N5, I1–I6).
- Robustness to **unknown host ETag and gzip behaviour** (owner).

## Industry Context & Research

**Key insight: make the validator content-derived, and let one small mutable file be all a client
polls.**
- Cargo's sparse index trusts host ETags and needed CDN invalidation to cope with stale edges.
- APT and TUF advertise child hashes in a small mutable root and fetch immutable, hash-named
  children (`research_catalog_sync_migrations.md` §B).
- OCX digests the exact served bytes at `/c/index.json` (OCX ADR D4).
- **conda CEP 16** is the closest shape: one mutable name→hash index plus immutable per-name
  shards, opt-in (`research_static_index_layouts.md`).

Precedent for the rest of this record:
- **Discovery by sibling path**: APT `by-hash` keeps the canonical path as a fallback.
- **An unpinned fleet means indefinite dual serving**: Cargo keeps its git index forever, while
  npm's forced cutover produced 404s.
- **Monolithic indexes fail at this size**: Helm's 13.4 MB `index.yaml` drove ~2 GB peaks.
- **Range requests fail on both target hosts**: GitHub Pages offsets index the gzip stream, and
  GitLab Pages ignores Range on compressed paths.
- **Build-cache poisoning reached production in 2026** (Cline, TanStack). Digest checks on
  restored bytes are necessary, not sufficient (`research_index_build_state_security.md` §1).
- **An unsigned root must not enforce hard monotonicity**: TUF fast-forward and APT `Valid-Until`
  wedges. grim adopts and logs (owner decision 2026-10-22, KISS; D5).

## Considered Options — the core layout and sync decision

Scores 1–5, higher is better. Figures are per client at 20,000 entries and unmeasured.

| Criterion | W | A. Flat + gzip + host conditional GET | B. Flat + content-validator root | C. Hashed tree under a 3-level root | D. Flat digest map | E. Namespace-sharded feeds |
|---|---|---|---|---|---|---|
| **S3**: one release < 100 KB, even with JSON uncompressed | 5 | 1 — 41 MB | 1 — 41 MB | **5** — ~37 KB | 1 — ~2 MB map | 2 — hot namespaces unbounded |
| Egress at the design point (a change in most hourly windows) | 4 | 1 | 1 | **5** | 2 | 3 |
| Egress with a quiet catalog, whatever the host does with ETags | 3 | 1 | **5** — ~0.5 KB | **5** — ~0.5 KB root | 2 | 2 |
| Principle 9 / compatibility | 5 | **5** | 4 — `c/root.json` and `c/f/` | 4 — five path families + retention | 4 | 2 — frozen shard key and segment names |
| Implementation + operating cost | 3 | **5** | 4 | 1 | 3 | 3 |
| Reversibility | 3 | **5** | 3 | 3 | 3 | 2 |
| **Weighted total (max 115)** | | 67 | 65 | **92** | 57 | 53 |

- **A** stays the fallback for indexes without a root, as the miss path (D4); on hourly redeploys
  host conditional GET rarely 304s.
- **B** is the shipped core: a quiet catalog costs ~0.5 KB whatever the host does with ETags. **C**
  alone meets S3 with uncompressed JSON. **D** puts 20,000 digests in the one mutable file and **E**
  freezes segment names: both rejected.

**Chosen: B.** C is deferred to [`adr_catalog_tree_sync.md`](./adr_catalog_tree_sync.md); its
triggers stay in D19.

## Considered Options — whole-index amendment (2026-10-21)

Each matrix scores 1–5, higher is better. **(owner)** marks the option the owner ratified on
2026-10-20. Where the recommendation changes an owner choice, the change is listed in the
Amendment record (C1–C14) and carried as an open question.

### M1 — Plane topology and root depth

| Criterion | W | A. 2-level root (2026-10-17 draft) | **B. Root → plane index → shard (owner)** | C. One mutable root per plane | D. One root inlining every plane's buckets |
|---|---|---|---|---|---|
| Quiet-catalog bytes per refresh under hourly stats churn | 5 | 2 — ~24 KB root | **5** — ~0.5 KB | 5 | 1 — ~72 KB, changes hourly |
| Catalog bytes per release | 4 | 5 | 4 — one more file | 5 | 2 |
| One generation names every plane | 3 | 4 | **5** | 1 — three files can skew | 5 |
| Permanent client-read surface | 4 | 4 | 3 — adds `c/p/` (pipeline-private since 2026-10-22) | 2 — three mutable files | 4 |
| Implementation cost | 2 | 4 | 3 | 3 | 4 |
| A new plane needs no new mutable file | 3 | 2 | **5** | 1 | 3 |
| **Weighted total (max 105)** | | 72 | **89** | 65 | 61 |

- **Risks (B):** one more file in the pipeline; `c/p/` is pipeline-private, so grim never reads it.
- **What the third level buys:** ~23.5 KB per refresh against A, about 5.6 GB per month per 1,000
  clients at 8 refreshes a day; A alone already meets S3 (Open question 9).
- **Reversibility:** one-way for every option once a client reads it.
- **Recommendation: B (owner), unchanged.** Since 2026-10-22 the plane index serves the enrich
  plane only; the catalog slot is reserved.

### M2 — Build-state store

| Criterion | W | **A. CI cache first, published site as fallback (owner, amended)** | B. Published site only, blobs on the site | C. Git "publish" branch (parked by owner) | D. Today's single checkpoint file |
|---|---|---|---|---|---|
| Scales to 20,000 (no file > 16 MiB) | 5 | 5 | 4 | 3 | 1 — breaks near ~200 packages |
| Poisoning resistance | 5 | 4 — bytes from cache, digests from the published root | 5 | 3 — CI holds a write token | 3 |
| Host budget impact | 4 | 5 — blobs off the site | 1 — ~3.7 GB site | 5 | 4 |
| Cost of a cache miss | 3 | 3 — one full re-enrich, ~38 min | 4 | 5 | 2 |
| Operational simplicity | 3 | 3 | 4 | 2 | 5 |
| GitHub and GitLab parity | 3 | 3 — GitLab cache is runner-local | 5 | 4 | 5 |
| **Weighted total (max 115)** | | **92** | 88 | 83 | 72 |

- **Amendment:** the cache holds **bytes only**; every digest comes from the last published root.
  Blobs are cache-only; the site fallback covers the planes, not the blobs (C7, D22).
- **Risks (A):** GitLab's runner-local cache misses more often; a miss costs build time, not
  correctness.
- **Reversibility:** two-way. The store is pipeline-private.
- **Recommendation: A (owner), amended.**

### M3 — Change detection

| Criterion | W | **A. Input hash + revalidate window, staggered buckets (owner)** | B. Main today: probe every package every build + ⌈N/7⌉ slice | C. Registry events (push) | D. Full re-enrich every build |
|---|---|---|---|---|---|
| Release latency bound | 4 | 4 — `revalidate` + build interval + build and deploy time | 5 — build interval + build and deploy time | 5 | 5 |
| Registry calls per build at 20,000 | 5 | 3 — ~42,900 at the default window, ~7% above B; fewer only with a raised `revalidate` | 3 — ~40,000 | 5 | 1 — ~60,000 |
| Catches companion-only changes | 4 | 4 — companion digest in the input | 4 | 2 — no GHCR events for third parties | 5 |
| Deterministic, byte-stable output | 3 | 5 | 3 — slice shifts as N changes | 3 | 4 |
| Implementation cost | 2 | 3 | 5 — on main | 1 | 5 |
| No host or registry feature needed | 3 | 5 | 5 | 1 | 5 |
| **Weighted total (max 105)** | | **83** | 85 | 67 | 82 |

- **Rescored 2026-10-21:** main probes every package outside the ⌈N/7⌉ slice
  (`IX/src/enrich/index.ts:334`), ~40,000 calls; A, with the same probe skip on its deep path (D23),
  makes ~42,900, about 7% more at the default window. The saving needs a raised `revalidate`. B
  leads by 2.
- **Risks (A):** a release waits up to one window plus one build interval (D23); a poisoned entry
  persists until rotation (≤ min(7 windows, 7 days), D23) or `--purge`.
- **Reversibility:** two-way; pipeline-private.
- **Recommendation: A (owner), kept** although B scores higher (Open question 15); a large index
  buys the call saving by raising `revalidate` (C2).

### M4 — Site search and browse

Moved to [`adr_index_site_at_scale.md`](./adr_index_site_at_scale.md).

### M5 — Retention across a full-site replace

| Option | Verdict |
|---|---|
| **A. The previous root's closure (owner, KISS 2026-10-22)** | `dist/c/` = the new root's closure ∪ the live root's closure; nothing older; one fallback generation |
| B. None | ~1 refresh in 6 falls back on hourly deploys |
| C. The former ledger with multi-generation windows and budgets | Rejected (KISS): a pipeline-private file, caps and a seed budget for a best-effort gain; design at `a5de87d3` |

- **Risks (A):** an older copy of a purged ref lives until the next build (D16, Open question 24).
- **Reversibility:** two-way; pipeline-private.
- **Recommendation: A (owner)**. Best-effort, not guaranteed (Open question 4).

### M6 — Blob placement versus host limits

Moved to [`adr_index_site_at_scale.md`](./adr_index_site_at_scale.md).

## Decision Outcome

### D1 — Baseline

Grim main (`fa92f264`) is the baseline. This ADR changes none of it: the 128 MiB cap, the
100,000-entry cap, the timeouts, the stale-serve semantics, `CATALOG_TTL_SECONDS = 3600`, the MCP
`limit` default and the shipped paging (`--offset`, `capped`).

The validator-gated floor stays the freshness ADR's D1, under the predicate restated in Metadata.
**No decision below adds a grim exit code.** Every new failure is a degraded refresh that serves
the cache and reports in `sources[]` (design doc §5.6).

### D2 — Published layout: one root, two planes and two flat blobs, beside `all.json`

**Kept and rejected cuts (owner decision 2026-10-22, [#216](https://github.com/grimoire-rs/grimoire/issues/216)).** The enrich plane stays published under `c/p/` and
`c/s/` (cut 4 not adopted). The root keeps naming the `c/f` blobs instead of `/all.json` and
`/stats.json` (cut 7 rejected), so `c/f/` and `sync.layout` stay.

| Path (relative to the directory holding `all.json`) | Mutable | Read by | Cap (decoded) |
|---|---|---|---|
| `c/root.json` | yes | grim, indexer | 64 KiB |
| `c/f/sha256/<64 hex>.json.gz` — the gzipped flat catalog or stats | no | grim, the site (stats) | inflated: `INDEX_BODY_LIMIT`, 128 MiB |
| `c/p/sha256/<64 hex>.json` — enrich plane index | no | indexer only | 256 KiB |
| `c/s/sha256/<64 hex>.json` — enrich plane shard | no | indexer only | 2 MiB |
| `c/q/…` — the site's search file ([site ADR](./adr_index_site_at_scale.md)) | no | the site only | — |
| `c/root.json.*` | — | **reserved**: detached signature (D15) | — |

**Root** (schema 1, ~0.5 KB; field rules: design doc §5.1):
- `schema`, `generation` (informational, D5), `generated_at`;
- `flat` and `stats`: `{sha256, size}` of the same build's exact `all.json` and `stats.json` bytes.
  `flat` is **`null` under `layout: flat`**; `stats` is `null` when the index publishes no stats;
- `planes`: always an object with members `catalog` and `enrich`, each `{sha256, size}` or `null`.
  `catalog` is reserved and always `null` until
  [`adr_catalog_tree_sync.md`](./adr_catalog_tree_sync.md) ships. grim ignores both members, so a
  change to the private plane never turns a root into a miss;
- `search`: `{sha256, size}` of the site's search file, or `null`. Additive and site-private: grim
  ignores it. It makes `c/q` reachable from the root, so retention covers it (D16).

**Flat and stats blobs**: `c/f/sha256/<hex>.json.gz` holds the gzip of the exact bytes of
`all.json` or of `stats.json`, written once and never rewritten, with no raw copy beside it. The
ref's `sha256` and `size`, and the file's name, cover the **inflated** bytes, so a flat digest is
the same on the blob path and on `/all.json`, and the inflated bytes are the frozen formats. The
producer gzips deterministically. A client reads the file as a plain file and inflates it when the
body starts with the gzip magic (`1f 8b`), aborting past the ref's size or `INDEX_BODY_LIMIT`; a
host or CDN that decoded it in transit is accepted; sha256 and size are verified either way (D5).
`flat: null` means "no flat blob": a client syncing the flat catalog through the root treats that
root as a miss (D4). The dossier's `c/b/` stays unused (D22). Rules: design doc §5.1, GR-R38.

**Enrich plane** (pipeline-private, read by the indexer only): a plane index naming 256 buckets by
the first byte of `sha256(ref)`, and shards carrying each package's data inline (design doc §5.1).

**Digest grammar**: `^[0-9a-f]{64}$`, lowercase only, over the exact served identity bytes,
checked before any URL or path is built. `src/oci/digest.rs:190` accepts uppercase, so it is not
reused. Every parent carries each child's `size`.

**`c` and `browse` are reserved** as top-level output segments in every layout (OCX D6; `browse/`
is the site's static listing). An operator-owned file (a `public/` layer or a theme page) that would
land under `dist/c/` or `dist/browse/` fails the build with exit 65 and a message naming it
(design doc IX-R02). A flat index that already ships either directory must move it (CD-9, Open
question 18).

**`all.json`, `stats.json` and `/p/<ns>/<name>/` are unchanged.** While `sync.legacy` is true (the
default, D3) every index serves `/all.json` and `/stats.json` at their frozen paths, in today's
serializer (D17). Both stay raw and gain deterministic `.gz` siblings, `all.json.gz` and
`stats.json.gz`, which GitLab serves to old and new clients alike through `Content-Encoding`; the
siblings are not a contract. Every pointer keeps its `/p/<ns>/<name>/` page, which is site output
and not governed by `legacy` (D17). The root's presence is not promised, like `stats.json`'s.

### D3 — Indexer opt-in: the `sync` block

```json
"sync": { "layout": "flat" | "anchored", "legacy": true, "revalidate": "1h" }
```

- **`layout`** (client-visible catalog shape):
  - absent or `"flat"`: clients behave as today; the root carries `flat: null` and no flat blob is
    written (the stats blob still is);
  - `"anchored"`: `flat` set;
  - `"tree"` is **reserved**: rejected with exit 65 until
    [`adr_catalog_tree_sync.md`](./adr_catalog_tree_sync.md) ships.
- **`legacy`** (bool, default `true`): `true` also writes `/all.json` and `/stats.json` at their
  frozen paths, with their `.gz` siblings, so every older grim keeps working. `false` publishes only `c/`; the
  `/p/<ns>/<name>/` pages are site output and stay. `legacy: false` with `layout: flat` is a config
  error: a flat-layout client has no `c/f` blob to read.
  - **Staged flip (owner decision 2026-10-22, [#216](https://github.com/grimoire-rs/grimoire/issues/216), cut 1).** `sync.legacy` stays, and the default is `true` now. It
    flips to `false` in the first indexer release after a grim carrying P2a becomes the supported
    floor. That release must also either change the default layout or allow `legacy: false` with
    `layout: flat`: today that pair exits 65, and `flat` is the default layout. grimoire-rs/indexer `docs: deprecate the true default of sync.legacy` (P2b branch, unpushed) carries the dated
    deprecation note of the `true` default.
- **`revalidate`** (D23): grammar `^[1-9][0-9]*(m|h|d)$`, 15 min to 7 days, default `"1h"`. Kept (owner decision 2026-10-22, [#216](https://github.com/grimoire-rs/grimoire/issues/216)): it is the interval for re-checking enrich buckets, unrelated to
  grim's root request (cut 2, D8).
- **The root is emitted in every layout** once the state store ships (D22), carrying the enrich
  plane and the `stats` ref; under `flat`, `flat: null` keeps every client on today's path (Open
  question 2).
- **Errors:** an invalid value, the reserved `tree`, or `legacy: false` with `layout: flat` fails
  the build with **exit 65**, the indexer's config-error class (`IX/src/cli/main.ts:88`). An older
  indexer warns about the unknown key and builds flat (`IX/src/config.ts:409-410`).
- **Opting in or out:** operator steps in design doc §13.1.
- **Producer bounds:** `build` warns at 80% of grim's `all.json` caps and loudly above them, never
  failing (design doc §5.9).

### D4 — Discovery: probe the root, once per refresh

grim derives the root URL with `reqwest::Url`, never with string slicing. A base or an `all.json`
locator probes `<index directory>/c/root.json`. Any other `*.json` locator, a query or fragment,
and git transport probe nothing (design doc §5.2).

The decision on the answer, one GET with its own 10 s timeout:
- **a valid schema-1 root with `flat` set, or a bound 304** → the anchored path (D5);
- **a miss** → today's `/all.json` path, unchanged: a 404 or 410; any other 4xx except 408 and 429
  (S3 and CDNs answer 403 for a missing key); a 200 that is not a valid schema-1 root; a valid root
  with `flat: null`. It logs at `debug` and is counted in `grim context` (D13);
- **a transport failure** (5xx, 408, 429, timeout, connect error, refused redirect) → a failed
  refresh once the source has adopted a root (`state.root != null`): the cached catalog is served
  with `ok: false`, and `/all.json` is not retried, since an outage is site-wide and a retry doubles
  the load. A source that never adopted a root (`state.root == null`) takes `/all.json`, so a probe
  failure cannot break a working flat index (Principle 9).

**The miss is not persisted.** A flat index costs one small GET per refresh. Full table: design doc
§5.2.

### D5 — Sync: verify before use, complete before current, the flat catalog is the floor

- **Verify before use.** Every child is stream-hashed under its cap (a gzip blob: its inflated
  bytes, D2) and matched to the parent's digest and size. Only then is it parsed, and only then
  persisted (temp + rename).
- **Anchored path:** a 304, or a root equal to the adopted one, changes nothing but `checked_at`; a
  root whose flat ref (digest and size) is unchanged is adopted with no download. Otherwise one
  immutable GET fetches the flat blob, verified against the ref (D2). **The root is adopted only
  after its flat blob verifies:** a root whose blob failed is never stored, so the next refresh
  re-fetches it and retries the blob. A mismatch or a 404 falls back to `/all.json` when the index
  serves it (`legacy`, D3); otherwise the refresh fails and the cache is served (design doc §5.3,
  GR-R36).
- **One aggregate deadline: 360 s per source refresh**, each request's timeout `min(own, remaining)`.
  **HTTP/1.1, one client per sync**: `http2` (a new crate) is not needed (design doc §6.2, §6.8).
- **Complete before current.** Commit order: `CatalogFile` → sidecar (design doc §6.3).
- **Equivalence** (the property tests pin): the flat blob and `/all.json` give equal entries except
  `fetched_at`, from one conformance fixture emitted by the indexer and checked into grim's tests
  (design doc IX-R42, GR-R26). **Dedupe:** the last occurrence in `all.json` array order wins
  (`index_source::project_entries`); `all.json` keeps every duplicate (D17).
- **`generation` is informational.** The producer keeps it rising (D17); grim never compares it: a
  verified root is adopted whatever its generation, and each adoption is logged at `debug` with it. Owner decision
  2026-10-22 (KISS): reverses Q7(a) (Open question 7). An older deploy served by an edge can flip a
  client back until the next refresh.
- **`--refresh`** sends no conditional headers, bypasses backoff and refetches the root.

### D6 — Record shape and key

Moved to [`adr_catalog_tree_sync.md`](./adr_catalog_tree_sync.md); the number is not reused.

### D7 — Client sync state lives in a sidecar, not in `CatalogFile`

**Location and contents.** The sidecar is `$GRIM_HOME/catalog/<h16>.sync.json`: private, versioned
and tolerant. It holds the binding, the layout, the adopted root's digest and validators (the only
`ETag` and `Last-Modified` it keeps), the flat digest and size, `checked_at` (the last refresh that
confirmed the catalog current), the stats digests (`advertised`, `applied`, `rejected`, D14) and the
failure state (fields and rules: design doc §5.4). Flat and stats blobs are parsed, never stored
raw; no root or child blob is kept locally. **No URL or locator is stored** (CWE-532).
`CatalogFile` keeps its byte shape.

**The binding is a digest, not a timestamp.** The sidecar applies only when its `catalog_sha256`
equals the SHA-256 of the `CatalogFile` bytes on disk; `null` means "no `CatalogFile` exists".
`built_at` has whole-second precision (`now_rfc3339` in `registry_catalog.rs`), so a timestamp
binding would survive two rewrites in one second. A grim release that changes how it projects
`all.json` bumps the sidecar `version`: an unchanged flat digest would otherwise never re-project.

**This reverses the freshness ADR D4's rejection of a sidecar, on new facts.** At 20,000 entries,
each new `CatalogFile` field costs every older binary a ~7 MB gzip cold rebuild. The extension's
managed grim and a PATH grim can share one `$GRIM_HOME`.

**Drop `deny_unknown_fields` from every catalog-cache struct**: `CatalogFile`, `CatalogEntry`,
`RatingSummary`, `DownloadSummary`, `DownloadVersion` (all in `registry_catalog.rs`).

### D8 — Conditional GET

**Cut by owner decision, [#216](https://github.com/grimoire-rs/grimoire/issues/216) (cut 2).** grim sends no conditional request and stores no validator; a 304 classifies as miss-200. The freshness ADR's validator-gated index floor (D1) has no validator to read and stays at the 3600 s TTL. `sync.revalidate` (D3, D23) is kept: it is the interval for re-checking enrich buckets and has no link to this request. The decision text below is kept as history.

- **Where:** `If-None-Match`, else `If-Modified-Since`, on `c/root.json` only, and only while the
  binding holds. **The root is the one conditionally fetched file.** The `c/f` blobs and every other
  `c/` child are immutable and never revalidated; the miss path's `/all.json` is fetched
  unconditionally, as today. Stored validators are capped at 256 bytes of visible ASCII; anything
  else is dropped, never persisted or replayed.
- **On a 304, a root equal to the adopted one, or a root whose flat ref is unchanged:** restamp
  the sidecar's `checked_at` only, adopting the new root's digest and validators when they differ.
  The `CatalogFile` is not rewritten: at 20,000 entries a rewrite re-serializes ≥ 41 MB per quiet
  refresh. A 304 means "keep what you have", never an authenticity signal; one that carries no
  validators is accepted against the stored one (design doc §6.4).
- **Unless the stats changed.** Ratings and downloads live in the `CatalogFile` entries
  (`CatalogEntry::rating` and `CatalogEntry::downloads`), and the stats rule (D14) runs on a quiet refresh too. When
  applying new stats changes any entry's rating or downloads, the refresh writes the joined entries
  and the new binding together, the stats digests included: a sidecar-only stamp would lose the new
  stats on restart, and with `applied` recorded they would never be fetched again. The cost is at
  most one `CatalogFile` rewrite per advertised stats digest (design doc §5.3, GR-R35).

### D9 — Redirects

One policy for every index GET: at most **5 hops**; **any https → http hop is refused**;
`referer(false)`; cross-origin https is allowed. **Not implemented on main**: `build_client`
(`index_source.rs:596`) sets no policy, so reqwest's default of 10 hops applies.

Rejected: `Policy::none()` (breaks Pages custom-domain and apex/www redirects); plain `limited(5)`
(permits a downgrade). Reserved: stripping custom auth headers cross-origin.

**Credentials.** Index GETs add no `Authorization` or `Cookie` of grim's own: no credential-store
lookup, no cookie jar. Userinfo in a locator is not refused or stripped: reqwest sends it as Basic
auth today, so an index that authenticates that way would break (Principle 9). It reaches only the
locator's origin and is never persisted or logged (D7; design doc §6.5).

### D10 — Git transport

The index leg keeps the full `--depth 1` re-clone; `ls-remote` stays dropped (owner ruling
2026-08-12). One hardened helper is shared with announce, with the timeout a per-call parameter.
**Not implemented on main:** `fetch_git` (`index_source.rs:789`) runs a plain shallow clone and sets
only `GIT_TERMINAL_PROMPT=0`.

- **The index clone gets every clause** of the helper, `--no-local` (the local-clone CVE class) and
  a 300 s timeout among them.
- **Announce gains one clause,** `GIT_ALLOW_PROTOCOL`, plus `kill_on_drop`: a write-side hardening
  change, listed in CD-8.
- **Errors** pass through one purpose-built redaction helper, shared with the HTTP leg: it strips
  userinfo, query and fragment and keeps scheme, host and port (CWE-532). `normalize_remote_url` is
  a provenance normalizer (it rewrites to `https` and drops the port) and is not reused.
- The no-`file` allowlist stays deferred to the owner; the freshness ADR's config-time shape gate is
  **withdrawn**: only `ext::` fails at clone time, and `--` neutralizes a leading `-`.

Exact clauses, argv, environment and research-row verdicts: design doc §6.7 and §7.3.

### D11 — Backoff after a failed refresh

A failed refresh backs off exponentially from 60 s to a 1 h cap, with ±20% jitter from the clock's
sub-second nanos (no `rand`), by saturating doubling capped at 3600 s; `failures` and
`last_failure_at` persist in the sidecar. The TTL cadence stays unjittered (freshness ADR D1). A
429's `Retry-After` lengthens the delay, never shortens it. During backoff the cache is served with
`sources[].ok = false` and an `error` naming the retry time; success resets it, `--refresh`
bypasses it, offline never reads it, and OCI sources share it. Formula, clock-safety clamps and
assertions: design doc §6.6.

### D12 — MCP `grim_search` limit: unchanged; the tool description steers

| Option | Verdict |
|---|---|
| Default 50 when `limit` is absent | **Rejected**: breaks the documented MCP parity contract and the "opt-in limit" (Principle 9) |
| A byte budget that truncates | **Rejected**: the same parity break |
| **Absent = unlimited; the description tells agents to pass `limit` and `offset`** | **Chosen**: additive; `total`, `truncated` and `capped` disclose any cut |

### D13 — Support diagnostic: `grim context` gains `registries[].catalog`

Always present; reads only the source's sidecar, never the network or the `CatalogFile`; `null`
until a grim with this change has refreshed that source. Members: `built_at`, `entries`, `layout`,
`generation`, `fallbacks`, `failures`, `last_error`, `retry_after` (shape, null rules and the
meaning of `built_at` and `fallbacks`: design doc §5.5). Ships in Phase 2a; the field names are a
one-way door. Rejected: a `grim catalog status` subcommand, which would freeze a new CLI surface.

### D14 — `stats.json`: a root ref to a `c/f` blob

- **No stats plane.** The root's `stats` ref names the build's exact `stats.json` bytes, gzipped, at
  `c/f/sha256/<hex>.json.gz`, in every layout. grim reads the blob. So does the indexer's seed
  (`IX/src/ratings/seed.ts:59-105`, the `ratings` and `downloads` commands and their CI steps),
  which takes a 404 on `/stats.json` for an empty seed and would wipe every carried rating on a
  `legacy: false` index. It reads the root's blob and falls back to `/stats.json` only after a root
  404 or a root naming none; a named blob it cannot read, verify or parse fails the seed, never an
  empty one (IX-R49). While `legacy` is true the same bytes are also served at `/stats.json`.
- **No timer: the advertised digest decides.** The sidecar tracks the *advertised* digest (the
  adopted root's), the *applied* one (the entries') and the last *rejected* one. On **every**
  refresh, a 304 or an unchanged root included, grim fetches the blob when the advertised digest is
  neither applied nor rejected, then verifies and applies it; compared only on a changed root, a
  deferred fetch would wait behind a 304 indefinitely. A blob that fails verification (404,
  mismatch, unparseable, a newer `schema_version`) is remembered as rejected and skipped until the
  root advertises another digest; a transport failure records nothing and retries on the next
  refresh. `--refresh` and `grim rate` force a fetch, a rejected digest included (Open question 3).
  Entries otherwise carry their previous rating and downloads; new entries stay unrated until the
  root names a new digest, and `grim rate` refetches once for an unrated row (GR-R10, GR-R20; design
  doc §5.3).
- **`stats: null`** advertises nothing: grim keeps the ratings and downloads it holds and fetches
  nothing until a root names a digest. Withdrawal clears no entry (design doc §14, V-39).
- **`generated_at` keeps its meaning.** The renderer carries it over (`IX/src/renderer/index.ts:485-510`),
  so the stats digest changes only when the ratings job runs. That run paces every client's download.
- **Named cost:** one stats download per client per ratings-job run that changes the digest, at most
  one per refresh (figures: Quantified Impact). The ratings ADR's revisit trigger governs that.

### D15 — Signing: reserved, not built

Reserved now, at zero cost:
- the root fields `schema`, `generation` and `generated_at`;
- the name `expires`, never emitted or enforced while unsigned;
- `c/root.json.*` for a detached signature;
- the rule that a verification key comes from client config or a built-in default, never the index.

### D16 — Retention across a full-site replace

**Kept (owner decision 2026-10-22, [#216](https://github.com/grimoire-rs/grimoire/issues/216), cut 3 rejected).** Retention on the content-addressed blobs stays. The
issue made its removal conditional on dropping `sync.legacy`, which stays (D3).

Pages hosts replace the whole site per deploy, and both edge-cache for 10 minutes (`max-age=600`),
so a client holding the previous root can still ask for files the new site no longer has.

- **`dist/c/` = the new root's closure ∪ the live (previous) root's closure.** Nothing older is
  kept. A closure is every file a root names: `flat`, `stats`, `search` and the enrich plane.
  Debian `by-hash` is the precedent: the archive keeps the previous Release's by-hash files.
- **Where the bytes come from:** `grim-indexer enrich` fetches them (online), from the blob store
  by digest first and from the explicitly configured `site` only on a miss, and verifies sha256
  and size; `grim-indexer build` places them in `dist/` (offline, D17). Digests come from the live
  root (D22) and paths only from the D2 templates, so a hostile root steers nothing.
- **Best-effort, not guaranteed** (Open question 4): a file that cannot be fetched or verified is
  skipped with a warning. A client whose file is gone gets a 404 and falls back to `/all.json`, or
  to its cache when `legacy` is false. Slower, never wrong data.
- **A failed or slow fetch warns and deploys anyway** (NFR-7): staging stops at `budget_end` +
  10 min (design doc §5.8, §6.10).
- **Takedown:** `--purge <ref|sha256>` removes the current copies (D17). An older copy of a purged
  ref lives until the next build, one build of lag (Open question 24).

Ships with Phase 2b. Algorithm and assertions: design doc §5.8.

### D17 — Indexer build pipeline

- **Two commands, one boundary.** `grim-indexer enrich` is online: it owns every grim call, the
  anchor read, the retained-file fetches, `--fresh`, `--purge`, bucket stamping and the blob-store
  writes. `build` and `dev` stay offline, the documented invariant
  (`IX/docs/explanation/index-vs-site.md:29`), and read only what `enrich` handed off; `state-key`
  is the only other online step. A failed `enrich` warns and the build still deploys, except exit 65
  when `--purge` or `--fresh` was passed: a mistyped purge must not deploy the data it was meant to
  remove. `enrich --seed` keeps its meaning, "seed from the published site", now read from the root
  and the enrich plane (design doc §3.7, §6.10, IX-R35, IX-R36).
- **The enrich plane replaces the checkpoint.** `enrich.json` packs base64 files and the marketplace
  into one document (`IX/src/enrich/checkpoint.ts:145-162`); the plane holds per-package data inline
  and companion files by blob reference (D22). Inline data is clamped on ingest, since today it is
  copied unbounded (`IX/src/enrich/index.ts:172-173`), and a shard that would still pass its 2 MiB
  cap degrades its largest entries to pointer-only (IX-R47). `layout: anchored` is no remedy, since
  every layout publishes the enrich plane. No exit code is new.
- **Deterministic serializer, new files only.** Plane indexes, shards and the root are written with
  keys in byte order and no insignificant whitespace; **`all.json` keeps today's serializer,
  one-space indentation and key order**, so there is no one-time change. The `c/f` blobs are the
  deterministic gzip of the `all.json` and `stats.json` bytes (D2).
- **Dedupe.** `all.json` keeps every pointer's element, duplicate refs included, and page routes
  come from the full pointer inventory, so no `/p/<ns>/<name>/` URL is lost. The enrich plane keeps
  one entry per `ref`: the last occurrence in sorted pointer-path order, the element grim's
  `BTreeMap` keeps from `all.json` (`index_source::project_entries`); the build warns, naming both paths.
  `validate` flags a pull request that adds a duplicate `ref` for manual review, under its existing
  contract (non-zero means review, never "reject", `IX/src/validate/index.ts:12-15`), since
  sorted-path last-wins would let a later-sorting pointer shadow another publisher's ref in grim's
  catalog. A write-side hardening change (CD-8).
- **Every `all.json` element field is bounded at emit**, through a key allow-list with a bound per
  key and a shared budget for unlisted keys, and loses one fixed set of code points, the set grim
  escapes in plain output (Security, T4). `all.json` still drops no key: it clamps the value in
  place, a value change only (CD-8). Bounds and the exact set: IX-R30.
- **`generation = min(max(now in UNIX seconds, published_root.generation + 1), 2^53 − 1)`**
  whenever a root is readable, `--fresh` included. A published generation above now + 1 day is
  ignored with a warning, so a fast-forwarded root cannot pin the producer.
- **Emit order:** the `c/f` blobs (`put_cas`), then the legacy copies and their `.gz` siblings (when
  `legacy` is true), the enrich plane and the site's search file, and the root last, only after
  every file it names exists and verifies (design doc §5.7, P-9).
- **Operator controls:** `enrich --fresh` ignores the cache and the published enrich plane but
  still reads the published generation. `--purge <ref|sha256>`: a 64-hex argument is a digest,
  anything else a ref; any other argument exits 65 before a request. Either forces a deep re-enrich
  of what it touched, outside the backoff and the time budget (a ref whose re-enrich fails is
  emitted pointer-only, so purged data is never carried forward), and starts a new cache epoch
  (D22); both are dispatch inputs of the rendered CI (design doc §6.10, IX-R10).
- **Job timeout and time budget:** rendered workflows set `timeout-minutes: 120` (GitLab
  `timeout: 2h`), and the GitHub templates pass it as `CI_JOB_TIMEOUT`, which GitLab sets itself. The
  budget is `min(85, job − 35)` minutes (85 when none is known); past it `enrich` starts no probe and
  no deep enrichment, carries the rest forward and lets the build deploy, so a cold build converges
  over runs. A pipeline left on GitLab's 60-minute default gets 25 minutes and warns "re-render
  required" (design doc §6.10, Open question 21).
- **Upgrade path**, named, not built: one batched probe returning both digests.

### D18 — Extension (`grimoire-vscode`)

1. **Release the 128 MiB buffer first**, live on the VS Code Marketplace and Open VSX before grim
   0.15.0 is tagged. It is on main (`VS/src/grim.ts:484`), unreleased. An old extension (v0.3.7
   caps grim's stdout at 16 MiB) with an uncapped grim overflows near ~8,000 rows, and the extension
   offers its users the latest grim release (`VS/package.json:331`, `VS/src/installer.ts:12`); no
   grim change prevents it.
2. **grim ships paging as 0.15.0**, matching the extension's gate `PAGING_GRIM_VERSION = '0.15.0'`
   (`VS/src/installer.ts:50`). The extension already loads every Browse page in the background on
   paging grims ([grimoire-rs/grimoire-vscode#30](https://github.com/grimoire-rs/grimoire-vscode/pull/30)).
3. **DOM windowing** in every density, mode and grouping (at most ~200 cards in the DOM, ARIA set
   size and position), with the host posting the list in bounded chunks. Gated like item 1:
   released before any index passes ~8,000 entries (VX-R03, VX-R04).
4. **Timeouts**: the default grim call timeout becomes 420 s (the 360 s deadline plus margin),
   except for calls that cannot refresh, which keep 120 s; an explicit timeout still wins (VX-R05).
5. **Source status**: "Incomplete results" follows `sources[].ok` and `error`, read additively, not
   any stderr (VX-R06).
6. **Voting on unrated rows** asks grim whether rating applies instead of skipping the row, gated
   on `RATING_GRIM_VERSION` (VX-R07).

Requirements and verification: design doc §17 (VX-R01…).

### D19 — Ship triggers and how each is measured

There is no telemetry, so every trigger is measured **host-side**. Pointers are versionless, so the
announce rate is measured through its proxy, the rebuild-time change rate.

| Trigger | Measured by | Fires |
|---|---|---|
| A | — | Trigger A: retired by the 2026-10-21 reset |
| B. Most hourly windows see an `all.json` change | The build log line: share of consecutive runs whose digest changed, over 7 days; > 50% | Start [`adr_catalog_tree_sync.md`](./adr_catalog_tree_sync.md) |
| C. Egress near the host cap | Host dashboard, else the formula in design doc §13.4 (client, stats and pipeline terms); > 50% of the cap | Start [`adr_catalog_tree_sync.md`](./adr_catalog_tree_sync.md) |
| D, E | — | The site triggers (search-file size, site budget) moved to [`adr_index_site_at_scale.md`](./adr_index_site_at_scale.md) |
| F. The flat blob nears the body cap | The build log line's inflated `flat bytes=`; > 50% of `INDEX_BODY_LIMIT` (64 MiB, ~32,000 entries) | Start [`adr_catalog_tree_sync.md`](./adr_catalog_tree_sync.md), which carries the cap raise |

`anchored` is the recommended layout for any host that can publish `c/`; no trigger gates it.

**Early warning (recorded recommendation, formerly Open question 3):** start
[`adr_catalog_tree_sync.md`](./adr_catalog_tree_sync.md) when B or C reaches **half** its threshold; F already sits at half the cap. Clients are unpinned, so relief lags by the upgrade rate.

### D20 — Old-client load: `legacy: false` is the lever

Older grims read only `/all.json` and `/stats.json`, so their load falls only as clients upgrade.
**`sync.legacy: false` is the in-band lever**: it stops publishing both frozen paths (D3, CD-10).
A 0.15.0 grim then fails to refresh and serves its cache, stale for good, and a cold install gets no
rows. A grim up to 0.14.3 has no stale-serve: its rows vanish once the 1 h cache expires, with the
error in `sources[]`. Neither reads `c/`. Set the lever only when no pre-P2a grim remains (0.14.x and
0.15.0), PATH-installed or extension-managed alike; shipping the P1 extension does not establish
that. While `legacy` is true both paths stay served indefinitely (owner). The default flips to `false` only on the staged condition in D3 (owner decision 2026-10-22, [#216](https://github.com/grimoire-rs/grimoire/issues/216), cut 1). The out-of-band levers are
the extension's managed grim, release notes and a CDN.

### D21 — The write side: two additive hardening changes

Announce writes a schema-1 pointer; the indexer accepts schema-1 pointers forever
(`IX/src/validate/core/metadata.ts:87`). The planes, the `c/f` blobs and retention are build outputs,
never committed. The two additive hardening changes (CD-8): `validate` flags a new duplicate `ref`
for review (D17), and announce's git gains `GIT_ALLOW_PROTOCOL` (D10).

### D22 — Build-state store: cache for bytes, the published root for trust

**(owner, amended.)**
**Kept (owner decision 2026-10-22, [#216](https://github.com/grimoire-rs/grimoire/issues/216)):** the published enrich plane (cut 4 not adopted) and `state_epoch`
with its weekly renewal (cut 5 not adopted).
- **Trusted manifest:** the last published `c/root.json` and the enrich plane it names, fetched by
  `enrich` over HTTPS from an explicitly configured `site`, never a default (an index that never set
  `site` would otherwise read, and republish, the first-party site's files), with no redirects,
  capped bytes and a cache-busting query. Every digest the build trusts comes from there, never
  from the cache.
- **Blob store:** a pipeline-local content-addressed store of companion files (README, changelog,
  contents) and of the enrich plane's own files, each blob capped at 1 MiB on ingest and re-hashed
  on every read (a mismatch counts as missing). It lives in CI cache only and is **never
  published** (C7); the dossier's `c/b/` name stays unused under the reserved `c/`.
- **Restore order per wanted digest:** the restored store → the site (plane files only; a 404 makes
  that one bucket cold) → a deep re-enrich from the registry (blobs).
- **Single writer:** only the default-branch **build** job restores and saves the cache, and only
  the blob-store directory, never `$HOME`, `$GRIM_HOME` or `~/.docker`. No job holding
  `pages: write` or `id-token: write` restores a cache; every other workflow and job (stats,
  validate, verify-ci) runs without one (the P0 PR). The save runs only when the job's earlier steps
  succeeded: a failed run publishes no root, so nothing published would name what it saved.
- **Keying and eviction:** an exact, epoch-bound key on GitHub with no `restore-keys`; a fixed key
  in the protected `pages` job on GitLab. `state_epoch` renews on `--fresh`, on `--purge` and weekly
  (Open question 23), so a poisoned entry stops being restored, at the price of a weekly cold
  build. A fixed GitLab key cannot carry the epoch, so there the incident step is the only eviction
  (`gh cache delete` on GitHub, "Clear runner caches" on GitLab). Key formats and the rendered
  rules: design doc §3.7.
- **Upgrade (Open question 19).** The first plane build, with no published enrich plane, is cold and
  re-dates `updated` for artifacts without `created`; an upgrade note says so.
- ~~**`enrich.json` sunset (owner):** the first plane release still writes `/enrich.json` in the old
  format, as a downgrade seed for an older indexer, **only when the file is ≤ 16 MiB**, since an
  older indexer refuses anything larger (`checkpoint.ts:185`). The next release stops writing it.~~
  **Dropped fully (owner decision 2026-10-22, [#216](https://github.com/grimoire-rs/grimoire/issues/216), cut 8).** The indexer never writes `enrich.json`, so no
  overlap release exists and the sunset happens now. P2e (IX-R34) is folded into P2b: grimoire-rs/indexer `feat: stop writing enrich.json` (P2b branch, unpushed).
  A downgraded indexer builds cold.
- **`site` unset, or neither `https` nor loopback:** no anchor and no retention. Every build is
  cold, warns, and deploys.
- **Rejected:** the git "publish" branch (parked by the owner); the single checkpoint file.

### D23 — Change detection: input hash and revalidate window

**(owner, amended.)**
**Kept (owner decision 2026-10-22, [#216](https://github.com/grimoire-rs/grimoire/issues/216), cut 6 not adopted):** detection stays per hash bucket, as below. The
per-package replacement needed cut 4, which is not adopted either. `revalidate` (D3) is the window below and is unrelated to grim's root request (cut 2, D8).
- `input = H(pointer bytes ‖ artifact digest ‖ companion digest or "none" ‖ enricher version)`.
- **Bucket gate:** a bucket is due when its pointer hash changed, its recorded enricher version
  differs from the running one, or its phase-staggered window elapsed; the windows are staggered
  across the 256 buckets, so each build probes a constant share (formula: design doc §6.10).
- **Per package in a due bucket:** two pooled grim calls (digest, companion digest); a deep enrich,
  three more calls (describe, companion fetch, contents fetch), only when `input` moved. A package
  already known to be deep skips the two probes, since `describe` and the companion fetch return
  both digests.
- **Rotation replaces the ⌈N/7⌉ slice.** Each bucket records when it was last fully deep-enriched;
  once its staggered rotation period `T = min(7 × revalidate, 7 days)` has elapsed, the next build
  deep-enriches it. Due buckets run oldest `min(checked, last-deep)` first, so after a time-budget
  stop the next build takes the skipped buckets first. A missed or irregular build skips no cohort,
  and a poisoned entry of a healthy ref persists at most T plus one build interval, in absolute time
  whatever `revalidate` is. A ref that keeps failing carries its last data forward for up to 30 days
  of continuous failure, then goes pointer-only (`data: null`, `files: {}`), so a vanished package
  stops showing stale text (IX-R51); `--purge` still removes it at once (Open question 25).
- **Stamping and failing packages:** a bucket is stamped only when every package in it succeeded or
  sits in its own failure backoff (exponential, capped at T, data carried forward), so one broken
  package never holds its bucket's rotation hostage. The backoff is armed only when fewer than half
  of the packages attempted in that build failed; otherwise the build is an outage and arms nothing.
  Exit 69 on `enrich` stays as today, with today's denominator, every package in the walk (design
  doc §6.10, IX-R40).
- **Release latency** is at most `revalidate + build interval + build and deploy time`; without
  stats there is no cron, so builds run on push only and the latency is unbounded until the next
  push.

### D24 — Site search and browse

Moved to [`adr_index_site_at_scale.md`](./adr_index_site_at_scale.md); the number is not reused.

### D25 — Site budget and blob placement

Moved to [`adr_index_site_at_scale.md`](./adr_index_site_at_scale.md); the number is not reused.

## Migration and rollout

**Release sequence.** Phase names are the same in the design doc (§12; steps in §13.2).
1. **P1, release what is on main:** first the extension's 128 MiB buffer and paging gate, live on
   both marketplaces; then grim 0.15.0 (pre-500, paging). 0.15.0 carries none of the
   layout-independent hardening (D9, D10, D11, GR-R23, GR-R27): that ships with P2a in
   [#210](https://github.com/grimoire-rs/grimoire/pull/210) (Open question 20).
2. **P0, indexer hardening,** independent of the layout: moved to
   [`adr_index_site_at_scale.md`](./adr_index_site_at_scale.md). It is released from indexer main
   after grim 0.15.0, which it needs, in the same release as the pool and carry-forward.
3. **P2a** grim and **P2b** indexer, in either order, except that P2a's gate needs P2b's
   conformance-tree half checked into grim first (IX-R42, GR-R26).
4. **P2d:** index operators re-render CI and opt into `anchored`. The first plane build is cold
   (D22).
5. **P2c** extension, any time after P1, but its windowing and chunked posts before any index
   passes ~8,000 entries.
6. ~~**P2e:** the next indexer release stops writing `enrich.json`.~~ **Folded into P2b (owner decision 2026-10-22, [#216](https://github.com/grimoire-rs/grimoire/issues/216), cut 8):** the indexer never writes `enrich.json`.
7. **Tree sync** is deferred: [`adr_catalog_tree_sync.md`](./adr_catalog_tree_sync.md)
   starts at the D19 early warning.

**Root grammar.** A Phase-2a grim validates `flat` and `stats` as null-or-ref and `planes` as an
object, and ignores `planes.catalog`, `planes.enrich` and `search`. A later tree reader needs no
Phase-2 change.

**Version mix** (the grim × index, extension × grim and indexer × previous site × CI matrices:
design doc §13.3). Every grim up to 0.15.0 reads `all.json` only, so only `legacy: false` affects it
(D20, CD-10). The one breaking extension mix, an extension ≤ 0.3.7 with a grim ≥ 0.15.0 on an index
past ~8,000 entries, predates this ADR; D18.1 releases the buffer first.

## Quantified Impact (estimates, 20,000 entries, per 1,000 clients × 8 refreshes/day, unmeasured)

| Metric | Today (grim main) | `anchored` |
|---|---|---|
| Catalog bytes per refresh, unchanged catalog, churning host | 7 MB gzip / 41 MB raw | ~0.5 KB |
| Catalog bytes per release | 7 MB / 41 MB | 7 MB (gzip file; 41 MB inflated) |
| Catalog egress per month, design point / quiet catalog | ~1.68 TB / ~1.68 TB | ~1.68 TB / ~120 MB |
| Stats per month | ~740 GB raw (every refresh) | ~21 GB gzip if the digest changes daily, ~170 GB if before every refresh (D14) |
| **Total client egress per month** (catalog + stats + ~5 KB TLS and headers per refresh) | ~1.85 TB on GitHub; ~10.5 TB on GitLab, which serves JSON raw | ~1.7–1.85 TB at the design point; ~22–170 GB for a quiet catalog, on either host |
| Pipeline egress per hourly build | the `enrich.json` seed, up to its 16 MiB read cap | warm store: < 1 MB (root, enrich plane index); retained files and plane files come from the store. Cold store: ~25 MB (the `c/f` and `c/q` files are gzip-only; the enrich plane's ~28 MB of JSON is raw on GitLab), ~18 GB a month if every build were cold |
| CI cost | — | up to ~17 GB/day of cache uploads against a 10 GB least-recently-used cache; GitLab `pull-push` re-uploads ~0.7 GB per job; for a private repository at hourly builds, ~31,000 runner-minutes a month typical (~43 min builds), ~41,000 worst (~57 min) |
| Cold start per client | 7 MB / 41 MB | 7 MB (gzip file) |
| Warm `grim search` | unmeasured at 20,000 | unchanged |
| Largest build-state file | `enrich.json`, ~1.7 GB | one enrich shard, ~110 KB |

## Consequences

**Positive**
- A quiet catalog is cheap on any host. `anchored` helps quiet catalogs only; a catalog that
  changes in most hourly windows needs the deferred tree sync
  ([`adr_catalog_tree_sync.md`](./adr_catalog_tree_sync.md)).
- A root miss or a flat blob that fails verification lands on `/all.json` while `legacy` is true;
  a 5xx or timeout on the root serves the cache (D4, D5). `legacy: false` retires the frozen paths
  in-band (D20).
- Gaps close: redirect downgrade, unbounded git clone, git env overrides, local-clone CVEs, retry
  storms, build-cache poisoning.

**Negative**
- Two permanent client-read path families (`c/root.json`, `c/f/`), a retention duty for the
  previous root's closure, and a root in every upgraded index, even one that stays flat.
- With `legacy: true` the site holds the flat catalog three times (`/all.json`, its `.gz` sibling
  and the `c/f/` blob), and retention keeps the previous root's closure, ~13 MB (up to ~41 MB after
  an enricher bump; site ADR S2).
- Per-source client state joined by a digest binding, and no rollback check (D5); stats egress
  follows the ratings-job cadence, not a 24 h cap (D14); retention is best-effort (D16).
- A cold blob cache costs one full re-enrich, and so do the first plane build (which also re-dates
  `updated`) and the weekly epoch renewal.
- CI cost grows: cache uploads, and runner minutes on a private repository (Quantified Impact).

**Risks**
- ~42,900 grim calls per build at a 1 h window (~27 min at 0.3 s per call, unmeasured); a cold
  cache, including the weekly epoch renewal, ~60,000 (~38 min). A build that outruns its time
  budget converges over runs (design doc §6.10, §8 NFR-1; Open questions 21, 27).
- The inflated flat blob must stay ≤ 128 MiB: about 64,000 entries under today's `INDEX_BODY_LIMIT`.
  The 100,000 ceiling needs the deferred tree ADR (Open question 11) and costs a client
  ~1.2–1.4 GB peak RSS per refresh (Open question 22).
- A host that rewrites the inflated JSON fails every flat-blob check, so its clients stay on
  `/all.json` (or on their cache when `legacy` is false), at the cost of a failed blob download per
  refresh; this is safe and visible in `fallbacks`, and memoising the failing digest is the named
  upgrade.
- GitHub auto-disables scheduled workflows after 60 days without activity.

## Security

Scoped to `arch-threat-model.md`. **Integrity, not authenticity.**
- **What the digest chain proves:** a client holds one consistent generation as served; the
  `CatalogFile` is tamper-evident through the sidecar's digest binding (I5). **What it does not
  prove:** anything about the origin. An unsigned, same-origin root lets the host mint a
  self-consistent generation (`adr_artifact_trust_model.md` decision 3, until D15).
- Catalog data never arms anything (I1); installs stay pinned by the lock's digest (T2).

| Attacker | Exposure | Control |
|---|---|---|
| T1-class index host or curator | hostile root, flat or stats blob; decompression bomb | decoded caps per kind, inflate aborted at the ref's size, size match, gzip only, verify-before-parse |
| T3: a repo-committed `[[registries]] index` | points grim at an attacker host | same caps; https → http refused; template URLs; git hardening (D10) |
| T4: record text reaching the terminal and MCP | plain output prints control characters | `all.json` elements emitted through a key allow-list, bounded, and stripped at emit of the IX-R30 code-point set (D17); plain output screened by a dedicated escape for exactly those code points (GR-R23, pre-existing gap); **residual**: a well-formed flag tag sequence (U+1F3F4, 3–7 tag digits or lowercase letters, U+E007F) is kept so subdivision flags render, so up to 7 hidden tag characters can ride behind a visible flag |
| On-path attacker | an `http://` locator, which `classify_index` accepts (`src/config/registry_resolve.rs:60`) | the scheme is the operator's explicit opt-in: grim rejects `insecure` on index entries because the locator carries its scheme (`src/config/project_config.rs:289-295`). An `https` locator gets TLS, and downgrade hops are refused (D9; Open question 5) |
| N2: a local process edits the cache | tamper | the sidecar applies only while its binding equals the `CatalogFile` digest; a mismatch discards it (D7) |
| A low-trust workflow writes the build cache | poisoned blobs or mapping | one writer, the default-branch build job (validate and verify-ci restore nothing); digests from the published root, re-hashed on read; a `state-key` value outside its grammar means the cold key; rotation; `--fresh`, `--purge`; a new `state_epoch` retires the key (D22, D23) |
| The same writer, through the archive itself | a cache restore extracts a tar before any digest check, so digests do not cover this route | only the single build-job writer, `cache-mode: none` elsewhere and the pinned cache path (D22) |

**Threat-model gap, follow-up (not edited here):** add T-rows for the index host/curator, the
on-path attacker and the build cache writer, and correct the stale `flate2` claim at
`quality-security.md:101-104` (design doc §7.5).

Argued deviations from the research (design doc §7.2): (1) rollback: no check; adopt and log (owner
decision 2026-10-22, KISS; D5); (2) the 7-day nudge is not added; (3) ETag ships in Phase 2a (D8);
(4) probe: a 200 garbage is a miss, and the miss is not persisted (D4); (5) the seen-signed bit
arrives with the verifier (D15).

## Constitution Check

| Principle | Status |
|---|---|
| 1–8 | Held: claims carry paths at named commits and the dossier corrections are listed; equivalence, version-mix and build-state assertions are tests; net security positive (D9–D11, D16, D22), integrity not authenticity stated; no persisted miss, subcommand or grim crate, and the tree waits for a trigger; one git helper, one root for every plane, one shard shape; the announce allowlist gap and the cache-writer gap close |
| 9 Preserve Compatibility | **Deviations below.** Frozen: `all.json`, `stats.json`, `/p/<ns>/<name>/`, served while `legacy` is true (D3). JSON additions follow the always-present-null policy. No new grim exit code |

CD-1 (a default MCP `limit`) was withdrawn in review on 2026-10-17 (D12); its number stays unused.
CD-7, the site's deviations, moved to the site ADR; its number stays unused.

| # | Deviation | Justification | Mitigation |
|---|---|---|---|
| CD-2 | An https index that redirects to http stops refreshing | A downgrade hop exposes the catalog on-path | Configure the final https URL; the cache keeps being served |
| CD-3 | A git clone over 300 s, or under 1 KB/s for 30 s, fails | An unbounded subprocess wedges the browse | Stale-serve plus backoff |
| CD-4 (info) | New permanent client-read paths under `c/`; the `c/f/` blobs are gzip-only | Additive, optional, presence not promised | `flat` sets the catalog fields null |
| CD-5 (info) | `deny_unknown_fields` dropped from the catalog cache | The cache is not a contract | — |
| CD-6 (info) | Stats are fetched when the root advertises a new digest, not on every refresh | The largest per-refresh transfer for a quiet catalog; a digest change is the only reason to pay it | `--refresh`; `grim rate` refetches (Open question 3) |
| CD-8 (info) | Indexer: `enrich.json` sunset; every variable `all.json` element field bounded and stripped of the IX-R30 code-point set at emit, so an over-long or control-laden value changes in `all.json` (its shape does not): `all.json` never drops a key, and an unlisted key past the shared budget keeps its place with its value clamped. Write side, two additive hardening changes: `validate` flags a new duplicate `ref` for review; announce's git gains `GIT_ALLOW_PROTOCOL` | None is a read contract; the write-side changes only add checks | ~~Overlap release~~ (cut: the sunset is immediate, owner decision 2026-10-22, [#216](https://github.com/grimoire-rs/grimoire/issues/216), cut 8); warnings; the existing manual-review path |
| CD-9 (info) | `c/` and `browse/` are reserved: an operator-owned file colliding with either fails the build with exit 65 and a message naming it | They hold generated content in every layout | Upgrade note and a CHANGELOG notice amending the theme-overlay tier (design doc §15, Open question 18) |
| CD-10 (info) | `sync.legacy: false` stops publishing `/all.json` and `/stats.json` | Opt-in per index; the default `true` keeps both; the in-band lever against old-client load (D20). The default flips to `false` on the staged condition in D3 (owner decision 2026-10-22, [#216](https://github.com/grimoire-rs/grimoire/issues/216), cut 1) | 0.15.0 grims fail to refresh and serve their cache; grims up to 0.14.3 lose the rows after 1 h (D20); set it only when no pre-P2a grim remains; `legacy: true` again restores both |

## Implementation Plan

1. [ ] **P1, release main:** extension D18.1 and D18.2 (VX-R01, VX-R02); grim 0.15.0 (GR-R11),
       without the layout-independent hardening.
2. [ ] **P0** moved out (Migration, step 2); released from indexer main with the pool and
       carry-forward (IX-R45).
3. [ ] **P2a, grim:** the P2a rows of design doc §18: D7 sidecar; D4/D5 anchored path with the gzip
       flat blob (GR-R38); D8; D13; D14; the D12 description; and, in
       [#210](https://github.com/grimoire-rs/grimoire/pull/210), the layout-independent hardening
       (D9, D10, D11, GR-R23, GR-R27).
4. [ ] **P2b, indexer:** the P2b rows of design doc §16.6: the root with `flat`, `stats` and
       `search`; the gzip `c/f/` blobs; `legacy` and its `.gz` siblings (IX-R52); the build order
       (D17); retention (D16); seeds on the stats blob (IX-R49); the 30-day cut (IX-R51).
5. [ ] **P2c, extension:** VX-R03…VX-R10 (design doc §17).
6. [ ] **P2d, index operations:** re-render CI; opt into `anchored`; watch B, C and F.
7. [ ] ~~**P2e, indexer:** stop writing `enrich.json` (IX-R34; tracked, not tested here).~~ Folded into P2b (owner decision 2026-10-22, [#216](https://github.com/grimoire-rs/grimoire/issues/216), cut 8): grimoire-rs/indexer `feat: stop writing enrich.json` (P2b branch, unpushed).
8. [ ] **Tree sync:** deferred; [`adr_catalog_tree_sync.md`](./adr_catalog_tree_sync.md)
       starts at the D19 early warning.
9. [ ] **Orchestrator:** the ADR index row; the threat-model rows.

## Validation

The test plan is design doc §14, whose test layers L1–L6 each hold a set of V-cases, and the
budgets are design doc §8, each NFR naming the case that verifies it. Gates before Accepted:

- [ ] Equivalence and version mix: V-1, V-2, V-4, the §13.3 matrices over {legacy only, `c/` +
      legacy, `c/` only}, the duplicate-upgrade fixture (V-30) and the shared conformance tree
      (V-28).
- [ ] Sync and hardening: V-3, V-5…V-12, V-23, V-31, and the reset's cases V-33 (a new stats digest
      behind a 304), V-34 (bad-blob recovery), V-36 (root probe failure modes), V-38 (sidecar
      binding), V-39 (stats withdrawal) and V-42…V-44 (the gzip blob).
- [ ] Incremental build, build state and the `enrich`/`build` boundary: V-15…V-17, V-20, V-24,
      V-29, V-32, V-35 (the stats digest golden), V-45 (the legacy `.gz` siblings) and V-47 (the
      30-day pointer-only cut).
- [ ] Benchmarks on synthetic 20,000 entries: V-13 (client), V-19 (indexer).
- [ ] Extension: V-21 (the §17 rows and the compat-marker sweep), before any index passes ~8,000
      entries.
- [ ] End to end: the `index-e2e` job (L6) builds with production `grim-indexer` at `legacy` true
      and false and syncs with grim and the pinned 0.14.3.
- [ ] Gates: V-22 (`task verify`; the indexer and extension suites).

**IDs moved or retired 2026-10-22 (KISS), never reused:** the canonical list is the design doc §18
footer (text of every one at `a5de87d3`).

## Open Questions

Resolved by the owner on 2026-10-22:

1. Moved to the site ADR (`adr_index_site_at_scale.md`): blob placement and the page cap.
2. Root and stats blob in every layout, and `anchored` ahead of the tree trigger: yes (D3).
3. Stats refresh timer: none. grim fetches the blob when the root advertises a digest it has
   neither applied nor rejected, behind a 304 included; `--refresh` and `grim rate` force (D14).
4. Retention guaranteed or best-effort: best-effort, previous root's closure only (D16). Retention itself is kept (owner decision 2026-10-22, [#216](https://github.com/grimoire-rs/grimoire/issues/216), cut 3 rejected).
5. Plain `http://` index locators: the scheme is the operator's explicit opt-in, so no residual and
   no follow-up (Security).
6. Moved to the site ADR: the `pull_request_target` checkout (P0).
7. Reversed: owner decision 2026-10-22 (KISS) reverses Q7(a). The soft ratchet is dropped:
   `generation` is informational and grim adopts and logs (D5). The narrowed 24 h retention is
   dropped with the ledger: retention is the previous root's closure (D16).
8. Closed by the reset: there is no stats plane; the stats are a root ref to a `c/f` blob (D14).
9. 3-level root over a 2-level one: keep the 3 levels (M1).
10. Moved to the site ADR: the search file's name.
11. Moved to [`adr_catalog_tree_sync.md`](./adr_catalog_tree_sync.md): the `INDEX_BODY_LIMIT` raise.
12. Moved to the site ADR: Pagefind and the empty-term fix.
13. The `sync.layout` value `root`: renamed `anchored`.
14. Moved to the site ADR: the plain `browse/<n>/` listing.
15. M3: keep the ratified A (rotation) (M3).
16. Precompressed siblings: the rule is lifted. The `c/f` blobs are gzip-only, and `/all.json` and
    `/stats.json` stay raw with deterministic `.gz` siblings (D2, D17).
17. Moved to the site ADR: where P0 is cut (now: released from indexer main after grim 0.15.0).
18. Reserving `c/` and `browse/`: amend the theme-overlay tier with a CHANGELOG notice; exit 65
    only on an actual collision (CD-9).
19. The first plane build: accept the cold re-date, with an upgrade note (D22).
20. Layout-independent grim hardening (D9, D10, D11, GR-R23, GR-R27): ships in [#210](https://github.com/grimoire-rs/grimoire/pull/210) with P2a; grim
    0.15.0 releases without it (Migration).
21. A partially enriched deploy while a cold build converges: accepted (D17).
22. RSS at the 100,000 ceiling: accepted; a streamed cache is the named upgrade.
23. `state_epoch` renews weekly (D22). Kept (owner decision 2026-10-22, [#216](https://github.com/grimoire-rs/grimoire/issues/216), cut 5).
24. An older copy of a purged ref lives until the next build: accepted (D16).
25. A ref that fails continuously for 30 days goes pointer-only; `--purge` stays the urgent path
    (D23, IX-R51).
26. Moved to the site ADR: Shiki classes against inline styles.
27. A cold build need not converge in one run (NFR-1; D23).
28. Moved to the site ADR: rendering markdown without Shiki.

## Links

- [`adr_catalog_freshness_revalidation.md`](./adr_catalog_freshness_revalidation.md) — partially
  superseded (Metadata)
- [`adr_registry_browse_filters.md`](./adr_registry_browse_filters.md) — D6 states the 500 window
- [`adr_artifact_trust_model.md`](./adr_artifact_trust_model.md) — decision 3
- [`adr_artifact_ratings.md`](./adr_artifact_ratings.md) — `stats.json` and its revisit trigger
- `docs/src/content/docs/stability.md:41` — the frozen package-index transport row
- [`adr_index_site_at_scale.md`](./adr_index_site_at_scale.md) — the moved site decisions and the P0
  hardening package
- [`adr_catalog_tree_sync.md`](./adr_catalog_tree_sync.md) — the deferred per-record tree sync

## Amendment record

### Corrections to the 2026-10-20 dossier (C1–C14)

The dossier facts the code or the research contradicts, each with its smallest correction; the full
table is in git at `a5de87d3`. C4 and C8 moved with the site; C1, C3, C5 and C10–C14 were folded
into D2, D3, D16, D17, D22 and D23 as written. The rows other sections cite follow.

| # | Dossier fact | Evidence | Correction |
|---|---|---|---|
| C2 | `input = H(pointer ‖ manifest digest ‖ enricher version)`; slice not mentioned | `IX/src/enrich/index.ts:319`, `:346` probe the companion digest; `:425` slices ⌈N/7⌉ | Add the companion digest; replace the slice with deterministic rotation (D23) |
| C6 | Enrich plane holds per-package data and files | `checkpoint.ts:158`, `:162` also pack the marketplace | The marketplace manifest is a blob referenced from the enrich plane index (D22) |
| C7 | "Previous state: CI cache first, published site as fallback", blobs under `c/b/` | `research_static_host_ci_limits.md` scenario table: ~3.7 GB with blobs published | Blobs are cache-only; the site fallback covers planes only (D22) |
| C9 | Per-package `checked` in the enrich shard entry | Every probe would rewrite every shard each build | `checked` moves to the enrich plane index, per bucket (D23) |

### Owner decisions on the #216 cuts (2026-10-22)

Recorded from [#216](https://github.com/grimoire-rs/grimoire/issues/216). The design doc and the plan carry the same marks.

| Cut | Decision | Effect |
|---|---|---|
| 1 `sync.legacy` | Staged flip | `sync.legacy` stays. The default is `true` now and flips to `false` in the first indexer release after a grim carrying P2a becomes the supported floor (D3). That release must also change the default layout or allow `legacy: false` with `layout: flat`, which exits 65 today. grimoire-rs/indexer `docs: deprecate the true default of sync.legacy` (P2b branch, unpushed) carries the dated deprecation note |
| 2 Root conditional GET | Applied | grim sends no conditional request and stores no validator (D8). `sync.revalidate` is kept and unrelated: it is the interval for re-checking enrich buckets (D3, D23) |
| 3 Retention | Rejected | Retention on the content-addressed blobs stays (D16) |
| 4 Published enrich plane | Kept | The plane stays published under `c/p/` and `c/s/` (D2, D22) |
| 5 `state_epoch` | Kept | The epoch and its weekly renewal stay (D22) |
| 6 Per-package detection | Kept | Per-bucket detection stays (D23); the per-package replacement needed cut 4 |
| 7 Root names `/all.json` directly | Rejected | The root keeps naming the `c/f` blobs, and `sync.layout` stays (D2) |
| 8 Overlap `enrich.json` | Dropped fully | The indexer never writes `enrich.json`. P2e (IX-R34) is folded into P2b, so the sunset happens now (D22). grimoire-rs/indexer `feat: stop writing enrich.json` (P2b branch, unpushed) |

---

## Changelog

| Date | Author | Change |
|------|--------|--------|
| 2026-10-17 to 2026-10-22 | architect (`/hex-architect`, xhigh) | Initial decision, the 2026-10-21 whole-index amendment and reset, the review rounds and the owner's answers: git history to `a5de87d3` |
| 2026-10-22 | owner (KISS) | KISS cut (owner): retention = previous closure; P3 tree sync deferred to adr_catalog_tree_sync.md; soft ratchet dropped (reverses Q7(a)); Q19 converter dropped. |
| 2026-10-22 | owner | Decisions on the [#216](https://github.com/grimoire-rs/grimoire/issues/216) cuts recorded: cut 1 staged flip, cut 2 applied, cut 3 rejected, cuts 4, 5 and 6 kept, cut 7 rejected, cut 8 dropped fully (P2e folded into P2b) |
