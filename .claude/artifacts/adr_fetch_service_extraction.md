# ADR: Neutral fetch-service seam + CWE-770 blob-size hardening

## Metadata

**Status:** Accepted
**Date:** 2026-07-10
**Deciders:** Michael Herwig; road-to-1.0 adversarial review
**Domain Tags:** architecture, security, api
**Related:** [adr_mcp_percall_scope_fetch_render.md](./adr_mcp_percall_scope_fetch_render.md)
(introduced `grim_fetch`/`grim_render` + the 8 MiB pre-download gate this
ADR completes), design record
[design_fetch_service_extraction.md](./design_fetch_service_extraction.md)
(implementation spec).

## Context

Porting `grim_fetch` to a CLI command welded `command/fetch.rs` onto
`src/mcp/fetch.rs`, creating a bidirectional `command ↔ mcp` module cycle:
the CLI imported MCP tool internals while `mcp/fetch.rs` reached back into
`command::` for scope/registry resolution.

Separately, the shared `OciAccess::fetch_blob` seam read the whole blob
body into an unbounded `Vec<u8>` before verifying its digest (CWE-770): the
8 MiB gate only checked the descriptor's *self-reported* `size`, so a
registry under-declaring size while serving a huge body defeated it. The
`grim install`/`update`/`add` path and the `grim_render` tool passed the
attacker-declared `layer.size` straight through with no policy cap at all.

The 1.0 freeze forces the layout+security decision now, while `grim fetch`
is newborn with zero external consumers.

## Decision Drivers

- 1.0 freeze: module layout is explicitly unstable, but the cycle ossifies
  once `grim fetch` gains consumers — cheapest to fix now.
- Complete the CWE-770 hardening the "harden now" decision requested, on
  **every** network-facing path, not just the lying-descriptor vector.
- Minimal correct ripple; mirror an existing proven pattern.

## Considered Options

**Seam:** (a) accept + document the `command→mcp` direction; (b) **extract
a neutral fetch core** mirroring `catalog/catalog_service.rs`; (c) defer.
**Cap:** (x) transport-integrity cap only (lying-descriptor); (y) **also
pre-gate the declared size at every caller**; (z) doc the residual + defer
install.

## Decision Outcome

**Chosen: (b) + (y).**

**Seam.** New crate-root `src/fetch.rs` — the role-analogue of
`catalog_service.rs`. The core takes already-resolved inputs
(`FetchScope` + `Arc<dyn OciAccess>`) and depends on neither `mcp` nor
`command`. `command::resolve_fetch_scope` single-sources the moved
resolution (forward `command→fetch` edge only); each front-end (CLI, MCP
fetch adapter, MCP render) resolves then calls the pure core. `command→mcp`
on the fetch path is eliminated; the pre-existing acyclic `mcp→command`
launcher direction remains.

**Cap.** Two ceilings, both enforced:
1. *Streamed-byte cap* — `fetch_blob` gains `max_bytes`; `registry_client`
   streams into a bounded `CappedSink` (tokio-only, no new dep) that aborts
   on actual bytes exceeding the cap → terminal `AccessErrorKind::OversizeBlob`
   (`ExitCode::DataError`, 65), before the digest re-hash.
2. *Pre-download policy gate* at every caller (the descriptor `size` is the
   `max_bytes` passed down, so both agree): bundle → existing
   `BUNDLE_LAYER_SIZE_LIMIT`; CLI/MCP fetch → `FETCH_BLOB_SIZE_LIMIT` (8 MiB);
   `grim_render` → `INSTALL_LAYER_SIZE_LIMIT`; install → `MCP_LAYER_SIZE_LIMIT`
   for mcp, new `INSTALL_LAYER_SIZE_LIMIT` (512 MiB) for skill/rule/agent →
   terminal `InstallErrorKind::OversizeLayer` (65). Makes the
   `OciAccess::fetch_blob` trait doc ("verified against policy caps before
   the call") true for every originator.

**Rationale.** (b) breaks the cycle with the smallest ripple that doesn't
create a `command↔fetch` cycle, and matches the codebase's one existing
neutral-seam precedent. (y) closes CWE-770 on the highest-traffic path
(`grim install`) rather than leaving a documented hole while the trait doc
claims full coverage.

### Consequences

**Positive:**
- `command↔mcp` fetch cycle gone; `src/fetch.rs` is a reusable neutral seam.
- CWE-770 closed on all network-facing paths (fetch, install, resolve,
  render); memory bounded by the policy cap regardless of registry honesty.
- Behavior-preserving extraction (Two-Hats): wire payload + report shapes
  unchanged; full acceptance suite green.

**Negative / deferred (tracked follow-ups, not this change-set):**
- The fetch core still returns `anyhow::Result` (carried verbatim from the
  pre-move `mcp/fetch.rs`), so some in-core `anyhow!` failures fall through
  to `ExitCode::Failure` (1). Visible asymmetry: the pre-download oversize
  gate in `fetch_artifact` exits 1 while the streaming `OversizeBlob` exits
  65 for the same condition. A typed `FetchError` (aligning the core with
  the `ResolveError`/`CatalogError` seams) would fix this — separate commit.
- No registry connect/read/idle timeout: a slow-drip body *within* the cap
  holds a connection. Pre-existing, unchanged here; needs a design call.

## Links

- [design_fetch_service_extraction.md](./design_fetch_service_extraction.md) — implementation spec, file-by-file
- [adr_mcp_percall_scope_fetch_render.md](./adr_mcp_percall_scope_fetch_render.md) — introduced the tools + the pre-download gate this extends

---

## Changelog

| Date | Author | Change |
|------|--------|--------|
| 2026-07-10 | road-to-1.0 review | Initial record (post-implementation) |
