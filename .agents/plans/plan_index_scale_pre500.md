# Plan: index scale fixes ahead of 500 entries

## Status

- **Plan:** plan_index_scale_pre500
- **Active phase:** 3 — Finalize
- **Step:** finalized
- **Last update:** 2026-10-17 (recomposed by /hex-finalize; release gate green)
- State:   done
- Updated: 2026-10-17
- Next:    merge the pull request (human)
- Tier:    high

---

## Overview

**Date:** 2026-10-17
**Branch:** `fix/index-scale-pre500` (primary checkout)
**Related ADR:** catalog-at-scale ADR, in design on `hex/adr-catalog-at-scale`;
amends `adr_registry_browse_filters.md` D6; ships part of the parked
`adr_catalog_freshness_revalidation.md` wave 2 (body cap, failed-rebuild fallback).

## Objective

Make an index source with more than 500 entries fully browsable, searchable, rateable
and checkable, and make large index fetches robust: compressed, bounded, not killed by
a total deadline, and not emptied by a failed refresh. One PR, additive only, with zero
new exit codes and no cache-format change.

## Context

`MAX_CATALOG_REPOS = 500` (`src/catalog/registry_catalog.rs:50`) was written for the
OCI `_catalog` walk (three round-trips per repository). It was copied onto index
sources with no rationale.

- `build_from_index` (`:831-834`) truncates in producer order before the query, the
  browse filter and the sort run.
- Rows past 500 are therefore unreachable by `search`, the TUI, MCP `grim_search`,
  `grim rate` and `grim status --check` (`CatalogScope::Complete`).
- The stderr warning (`src/command/search.rs:176`) advises narrowing the query, which
  cannot help.

The transport has three more problems:
- reqwest is built without gzip (`Cargo.toml:58`);
- `fetch_http` reads an unbounded `.bytes()` under a 30 s **total** deadline
  (`src/catalog/index_source.rs:34,585-605`);
- a failed rebuild returns `Err` even when the cache it just read is usable
  (`registry_catalog.rs:648,685`).

Measured on a large synthetic index (2026-10-17):
- the index is 2.07 KB/entry raw and compresses about 6x under gzip;
- an uncapped warm search takes 0.21 s / 170 MiB, and a cold one 0.36 s;
- `grim search --format json` output exceeds the extension's 16 MiB buffer.

## Key decisions

**D1 — Uncap the index; keep the OCI cap.**
- `build_from_index` drops `.take`/`truncate` and records `truncated: false`. The
  `scope` filter is unchanged.
- `MAX_CATALOG_REPOS` and `Catalog::build` are unchanged. Their docs now say they
  apply to the `_catalog` walk only.
- ~~No self-heal for an old capped cache.~~ *Revised in review:* the index loader
  treats a cached index catalog with `truncated: true` (only an earlier grim writes
  one) as needing a rebuild. If that rebuild fails, the stale rows are served with a
  warning that names `grim search --refresh`, not "use a package index".

**D2 — Search envelope**
`{"items": [...], "sources": [...], "total": <int>, "truncated": <bool>}`.
- `total` counts every matching row before `--limit`. The browse filter, deprecated
  visibility, the query and the relevance cutoff all apply first.
- `truncated` is `true` when `items` is not the complete answer. That happens when
  `--limit` cut it (`items.len() < total`), or when an OCI source's listing was capped
  at build time (then `total` is a lower bound).
- Both fields are always present. `skip_serializing_if` is banned in `src/api/`.
- `--limit <N>` takes a `NonZeroUsize`; `0` or a non-integer is a clap usage error,
  exit 64. It applies after ranking and `--sort`.
- MCP `grim_search` takes an optional `limit`. Its payload stays byte-identical to the
  CLI's.
- `SearchReport::new(items, sources)` keeps its signature, defaulting to
  `total = items.len()` and `truncated = false`. One builder method sets both from
  `search::run`, which avoids touching 33 test call sites. `CatalogGroup` gains no
  field.

**D3 — Honest cap warning.**
- The trigger is unchanged (`any_truncated() && !parsed.is_empty()`). Only OCI sources
  can truncate now.
- The new text names the capped source(s) and says a narrower query cannot reach past
  the cap. It points to an index source or a narrower `oci` locator.
- No new stderr appears on an unqueried browse, because the VS Code extension shows any
  stderr as "Incomplete results".

**D4 — Transport (`src/catalog/index_source.rs`).**
- ~~reqwest gains the `gzip` feature.~~ *Revised in review:* the feature unifies
  crate-wide and turned on decoding in the OCI, forge and auth clients. grim instead
  sends `Accept-Encoding: gzip` on the two index requests only and decodes with
  `flate2` (multi-member, capped, on the blocking pool). Other encodings fail the fetch.
- A single helper `read_capped(response, limit)` loops `chunk()` and counts
  **decoded** bytes. `Content-Length` is unset after decoding, so it cannot be used.
  - Over the limit it returns `CatalogError::index_fetch` with an `io::Error::other`
    source.
  - It replaces `.bytes()` in `fetch_http` and `fetch_stats`, and it doubles as the
    gzip-bomb guard.
- `INDEX_BODY_LIMIT = 128 MiB`. The freshness ADR's 32 MiB is too small, since a large
  index comes to tens of MB raw.
- *Added in review:* `INDEX_ENTRY_LIMIT = 100_000`, an abuse bound (far above any
  real index). A byte cap alone allowed ~11 GB RSS from minimal entries. Over it
  the fetch fails; it never truncates.
- `http_client()` uses `connect_timeout(10 s)`, `read_timeout(30 s)` (stall) and
  `timeout(300 s)` (overall ceiling). These are named constants and replace
  `HTTP_TIMEOUT`.
- Out of scope: brotli, the redirect policy, ETag/304, and git-transport timeouts.

**D5 — A failed refresh serves the cached catalog.**
- `Catalog` gains an in-memory-only `refresh_error: Option<String>`. `CatalogFile` is
  unchanged, so the field never reaches disk.
- Both coordinated error arms change: OCI `load_or_refresh_coordinated` and
  `load_or_refresh_index_coordinated`. When pre-flight read a cache, they dispose the
  guard and return `Ok(previous.with_refresh_error(kind))`. With no cache they return
  `Err`, unchanged.
  - The OCI arm binds `previous` from `PreFlight::Rebuild`.
  - The index arm keeps `previous` owned and lends `&entries` to the build.
- In the `catalog_service` Ok arm, `served_offline = offline || refresh_error.is_some()`,
  `error = refresh_error`, and the rows are built from the cached catalog. It warns
  `catalog for source '<x>' unavailable: <kind>; serving the cached catalog built <built_at>`.
  That prefix matches the existing line.
- In JSON the result is `sources[].ok: false` with `error` set, and the cached rows in
  `items`. The row set is not a contract (`stability.md` § search relevance).
- The cache is not restamped, so the next command retries once the cache has expired
  (or on `--refresh`). `rate` and `status --check` consume the stale rows too.

## Backwards compatibility

| Pair | Effect |
|---|---|
| New grim + cache from old grim | `CatalogFile` format unchanged. A capped index cache heals within the 1 h TTL or with `--refresh`. |
| Old grim + cache from new grim | Format unchanged. Old grim renders the uncapped rows, because the cap runs only at build time. |
| JSON consumers | `total` and `truncated` are additive. The extension ignores unknown keys. `--limit` is opt-in. |
| MCP | `limit` is optional. MCP and CLI payloads stay identical. |
| Index hosts | Nothing is required of them. A host that ignores `Accept-Encoding` still gets plain JSON. |
| Failed refresh | Before: an empty group with `ok: false`. After: cached rows with `ok: false`. Exit 0 both ways. Noted in `upgrading.md`. |

**Exit codes — none added.**
- `--limit 0` exits 64 (clap usage error).
- MCP `limit: 0` returns an invalid-params tool error.
- Over-cap, decode and stall failures are `IndexFetch` (the 69 class). A browse still
  degrades them to `ok: false` and exit 0.

## Pipelines

Hub file: `Cargo.lock`. It is regenerated once at integration, minimally (no
`cargo update`), and never committed by a pipeline. Pipelines commit with
`git -C <worktree> commit --signoff`.

| Pipeline | Status | Owns | Depends on |
|---|---|---|---|
| A catalog | merged | `src/catalog/registry_catalog.rs`, `src/catalog/catalog_service.rs`, `src/config/registry_filter.rs` (comment at `:119` only) | contract wave |
| B search | merged | `src/command/search.rs`, `src/api/search_report.rs`, `src/mcp/server.rs`, `src/mcp/tool_args.rs`, `test/tests/test_mcp.py` | contract wave |
| C transport | merged | `Cargo.toml`, `src/catalog/index_source.rs` | contract wave |
| D docs | merged | `docs/src/content/docs/{commands,configuration,json-interface,stability,upgrading,hosting-an-index,package-index}.md`, `catalog/skills/grim-usage/references/{registries,consume}.md`, `.claude/rules/subsystem-cli-commands.md`, `.agents/adr/adr_registry_browse_filters.md` | contract wave |

The `grimoire-vscode` change runs in parallel as a separate PR, outside this plan's
feature branch. It raises `maxBuffer` from 16 to 128 MiB in `src/grim.ts` and reports
an overflow clearly.

## Implementation steps

### Contract wave (feature branch)

- [x] W0.1 Lift `_package`, `_write_all_json`, `_index_config` and the `http_index`
      server from `test/tests/test_index_source.py` into `test/src/index_fixture.py`.
      This is a move only; `test_index_source.py` imports them.
- [x] W0.2 Write `test/tests/test_index_scale.py`, red:
  - a 600-row HTTP index lists 600 rows with `total: 600, truncated: false`;
  - a query and an `include` glob each reach row 550;
  - `grim rate --dry-run <row 550>` with a stats sidecar rating exits 0 (79 today);
  - `--limit 5` returns 5 items with `total: 600, truncated: true`;
  - `--limit 0` exits 64.
- [x] W0.3 Write `test/tests/test_index_transport.py`:
  - a gzip-encoded `all.json` lists its rows, and the request carried
    `Accept-Encoding: gzip` (red);
  - a gzip bomb (about 130 KB that decodes past 128 MiB) gives `ok: false`, exit 0, and
    no OOM. This is a regression guard and is green on both sides.
- [x] W0.4 Write `test/tests/test_index_stale.py`, red:
  - warm the cache, then make `all.json` return 500. `search --refresh` must list the
    cached rows with `sources[0].ok == false` and `error` set, and stderr must name
    "serving the cached catalog";
  - the cold-cache failure path stays unchanged.
- [x] W0.5 `test/tests/test_mcp.py`: `grim_search` with `limit` caps `items`, and the
      MCP and CLI payloads stay identical (red).
- [x] W0.6 Rebuild `test/bin/grim` (a copy) and run the new files. Confirm each red test
      fails for its intended reason, then commit `test: …`.

### Pipeline A — catalog

- [ ] A1 Uncap `build_from_index` and scope the `MAX_CATALOG_REPOS` docs to the
      `_catalog` walk. Fix the comment premises at `catalog_service.rs:484-487` and
      `registry_filter.rs:119`.
  - Red first: a unit test that a 600-entry host yields 600 untruncated rows, placed
    beside `build_caps_repository_count`. The OCI cap tests stay green.
- [ ] A2 Implement D5 stale-serve on both arms and in the `catalog_service` Ok arm.
  - Red first, beside `coordinated_releases_lock_on_build_error`:
    - warm cache: the index arm and the OCI arm each return the cache plus
      `refresh_error`;
    - cold cache: both still return `Err`;
    - the lock is released in every case;
    - the service group carries rows, `error` and `served_offline`.

### Pipeline B — search

- [ ] B1 Envelope `total` and `truncated` (D2).
  - Update the `empty_results_serialize_as_empty_items` tripwire.
  - Add tests that `total` counts rows before the limit, and that `truncated` reflects
    a source cap.
- [ ] B2 `--limit` (clap `NonZeroUsize`), applied after `order_results`.
  - Test that the limit applies after sort, and that it appears in the help text.
- [ ] B3 MCP `limit` in `SearchToolArgs`.
  - Schema test.
  - The description mentions `limit` and `total` and keeps the phrases "browse filter"
    and "direct reference".
- [ ] B4 Warning text (D3), with a unit test pinning the new wording.

### Pipeline C — transport

- [ ] C1 reqwest `gzip` plus `read_capped` in both fetches.
  - Unit test the cap with a small injected limit over a raw-socket host.
  - Assert the `Accept-Encoding` header.
- [ ] C2 Connect, stall and overall timeouts.
  - The stall test uses a paused tokio clock if reqwest honours it. Otherwise it uses
    a client built from short injected durations.

### Pipeline D — docs

- [ ] D1 Scope every "500 / narrow the query" statement to `oci` sources:
  - `commands.md:1220-1226`
  - `configuration.md:596-609`
  - `registries.md:370-373`
  - `subsystem-cli-commands.md` (`grim search` row)
- [ ] D2 Document `--limit`, `total` and `truncated`:
  - `commands.md` search section and MCP table row `:2593`;
  - `json-interface.md`: envelope siblings `:74-77`, `:97`; rewrite `:106-121`; a new
    `{#search-total}` section; MCP parity `:884-889`;
  - `registries.md:798`;
  - `consume.md:328-330`.
- [ ] D3 Stale-serve wording:
  - the `json-interface.md` sources section;
  - the TTL/offline sentences in `package-index.md:60-62` and `hosting-an-index.md:151`;
  - the warning text in `commands.md:1284` and `registries.md:691`.
  - Also add a gzip hosting tip in `hosting-an-index.md`.
- [ ] D4 `upgrading.md` "Smaller notes" bullets for the recall change and the
      stale-serve change. Add the additive example to `stability.md`.
- [ ] D5 Append an `> **Amended 2026-10-17**` block to
      `adr_registry_browse_filters.md` D6, and amend the rejected alternative at `:679`.
- [ ] D6 Run the catalog drift review (`catalog/README.md` › keeping content honest).
  - Keep every existing `{#anchor}`, use reference-style links, and cap sentences at
    25 words.

### Integration

- [ ] I1 Merge A → C → B → D (serialized, `--no-verify`).
- [ ] I2 Regenerate `Cargo.lock` minimally and commit it as `chore(deps): …`.
- [ ] I3 Run `task --force verify` and `task docs:check` concurrently with one
      `/hex-review` round over `main..HEAD`.
- [ ] I4 Run at most one fix pass, verified by the gate. There is no second review
      round.

## Verification

- The new acceptance files are red on the `main` binary and green on the branch.
- `task --force verify` is green: clippy, nextest, `catalog:verify`, `test:parallel`,
  `claude:tests`, cargo-deny licences.
- `task docs:check` is green.
- Manual check on a large synthetic gzip index:
  - wall time and RSS against the 0.36 s / 170 MiB cold baseline;
  - `--limit 20` reports the full `total`;
  - with the server stopped, the cached rows come back with `ok: false`.

## Deferred (catalog-at-scale ADR)

- A default MCP `limit`.
- ETag/304 revalidation.
- The redirect policy.
- Git-transport timeouts and `kill_on_drop`.
- Backoff after a failed refresh.
- The extension passing `--limit`.
