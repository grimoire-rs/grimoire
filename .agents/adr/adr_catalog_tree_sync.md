# ADR: Catalog tree sync — a hashed record tree under the index root (deferred)

## Metadata

**Status:** Deferred
**Date:** 2026-10-22
**Deciders:** Michael Herwig (maintainer)
**Parent:** [`adr_catalog_at_scale.md`](./adr_catalog_at_scale.md) — the anchored core (root, flat
blob, stats, enrich plane, previous-closure retention) ships without this ADR.
**Prior design:** the full Phase 3 design (options matrix C, sync paths, store, requirements) is in
commit `a5de87d3`, `.agents/adr/adr_catalog_at_scale.md` and
`.agents/specs/design_catalog_at_scale.md`. Start from there; do not re-derive it.

## Context

The anchored core downloads the whole flat catalog whenever its digest changes. At the 20,000-entry
design point that is ~41 MB inflated per changed release, so a catalog that changes in most hourly
windows costs every client a full download. Only a per-record tree meets driver S3 (one release under
100 KB with uncompressed JSON). The owner deferred it on 2026-10-22 (KISS): the core is useful on
its own, and the tree adds five path families, a local store and two sync paths.

## What the deferred design was

- `c/root.json` `planes.catalog` names an immutable plane index of 256 hash-bucket shards; each
  shard names immutable records under `c/r/sha256/`, keyed by `ref`.
- grim takes a delta (changed shards, then changed records) or a bulk path (the flat blob, then every
  shard) over HTTP/1.1 with 8 connections, persists a local tree store, and falls back to the flat
  blob on any child failure, with a tree-retry backoff.
- The indexer dedupes shards with grim's last-wins rule and emits one canonical-JSON conformance tree
  that both sides test against.

## Reserved in the core (do not reuse)

- The root member `planes.catalog` (always `null` until this ADR ships; clients ignore it).
- The path segment `c/r/` and the `sync.layout` value `tree` (rejected with exit 65 until then).
- IDs moved here from the parent's design doc on 2026-10-22: GR-R03, GR-R04, GR-R24, GR-R25,
  GR-R32, IX-R25, NFR-16, NFR-25, P-2, P-6. Their text is at `a5de87d3`; none is reused.

## Triggers to start this ADR

Measured host-side; B, C and F are the parent's D19 rows:

- **B.** Most hourly windows see an `all.json` change: share of consecutive builds whose flat digest
  changed, over 7 days, above 50%.
- **C.** Egress near the host cap: above 50% of the cap.
- **F (size).** The inflated flat blob passes 64 MiB, half of `INDEX_BODY_LIMIT` (128 MiB), at
  ~32,000 entries. At ~2 KB per entry the cap itself holds roughly 64,000, short of the 100,000
  ceiling.
- **Early warning:** B or C at half its threshold starts the work, since clients upgrade slowly; F
  already sits at half the cap.

## Carried decisions

- **`INDEX_BODY_LIMIT` to 256 MiB** (formerly GR-R25, Open question 11): needed for a cold fetch at
  the 100,000 ceiling (~207 MB inflated); a hostile body then costs ~1.4 GiB RSS.
- Retention for records and shards follows the parent's rule (previous root's closure) unless this
  ADR argues otherwise.

## Changelog

| Date | Change |
|---|---|
| 2026-10-22 | Created as a deferred stub when the owner cut Phase 3 from `adr_catalog_at_scale.md` (KISS). |
