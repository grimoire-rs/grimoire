# Design Record — Fetch-Service Extraction + CWE-770 Blob Cap

Status: Implemented (commits a7270f4 + e4f3fb6 + road-to-1.0 review fixups).
Decision recorded in `adr_fetch_service_extraction.md`; this file is the
file-by-file implementation spec.
Scope: break the `command ↔ mcp` fetch cycle by extracting a neutral fetch
core (mirroring `catalog/catalog_service.rs`); thread a real byte cap through
`OciAccess::fetch_blob` (CWE-770). Two code commits, Two-Hats separated.

References read: `command/fetch.rs`, `mcp/fetch.rs`, `api/fetch_report.rs`,
`mcp/tool_args.rs`, `mcp/render.rs`, `mcp/server.rs`, `command/scope_resolution.rs`,
`command.rs`, `catalog/catalog_service.rs`, `oci/access.rs`,
`oci/access/registry_client.rs`, `oci/access/cached_access.rs`,
`oci/access/memory_registry.rs`, `oci/access/error.rs`, `error.rs`,
`install/installer.rs`, `resolve/resolver.rs`,
`external/rust-oci-client/src/client.rs`.

---

## 1. Neutral module

**Path/name: `src/fetch.rs`** (crate-internal `mod fetch;` in `src/main.rs`,
inserted between `mod error;` (24) and `mod install;` (25) — alphabetical,
matching sibling convention; `mod`, not `pub mod`).

**Justification vs catalog's placement.** `catalog_service.rs` sits *under*
`src/catalog/` because a catalog subsystem already existed with sibling files
(`registry_catalog.rs`, `search_match.rs`, `catalog_error.rs`). There is no
`src/fetch/` subsystem — the fetch core is one cohesive module — so grim's
flat "one concept per file, named module, no `mod.rs`" convention
(`arch-principles.md`) puts it at the crate root as `src/fetch.rs`. It is the
role-analogue of `catalog_service.rs`: the single neutral seam every front-end
depends *down* onto. Promote to `src/fetch/` only if it later grows siblings
(YAGNI now).

**Items that MOVE into `src/fetch.rs` from `mcp/fetch.rs`** (verbatim bodies,
except the cycle-breaking signature changes in §2):

| Item | Kind | Note |
|---|---|---|
| `FETCH_BLOB_SIZE_LIMIT` | `pub const u64` | 8 MiB |
| `FETCH_DOC_SIZE_LIMIT` | `pub const usize` | 256 KiB |
| `TRUNCATION_MARKER` | `const &str` | private |
| `FetchedArtifact` | `pub struct` | unchanged fields |
| `FetchFileEntry` | `pub struct` | unchanged |
| `FetchReport` | `pub struct` | unchanged (MCP-shaped, `skip_serializing_if` — exempt per subsystem-cli-api.md) |
| `fetch_artifact` | `pub async fn` | signature changes (§2) |
| `fetch_with_limit` | `pub async fn` | signature changes (§2) |
| `project_index`, `entry_content`, `cap_content` | `fn` | private, move verbatim |
| the `#[cfg(test)] mod tests` block | tests | move; update call-sites (§2) |

**Item that STAYS in `mcp/fetch.rs`:** `fetch(ctx, &FetchToolArgs) ->
anyhow::Result<FetchReport>` shrinks to a thin MCP adapter (§2) that
`server.rs:107` still calls. `FETCH_DOC_SIZE_LIMIT` is referenced from the
core (default doc cap) and re-exported? No — the MCP `fetch` adapter reads
`crate::fetch::FETCH_DOC_SIZE_LIMIT` directly.

---

## 2. Cycle-break — THE critical decision

### Diagnosis of the reverse edge

`mcp/fetch.rs::fetch_artifact` reaches back into `command::` for exactly two
kinds of thing:

1. **Scope→registry resolution** (lines 142–167): `scope_resolution::resolve_in`
   + `registries_for_scope` / `resolve_default_registry` /
   `global_config_default`, with the `registries_global_fallback` /
   `primary_registry_global_fallback` degraded path.
2. **Plumbing helpers**: `command::grim` (error-wrap into `crate::error::Error`
   for exit-code classification) and `command::access_seam` (builds
   `Arc<dyn OciAccess>` from `ctx`).

The input type `ScopeToolArgs` is an **mcp** type (`mcp/tool_args.rs`).

### Decision (mirror the catalog precedent exactly)

`catalog_service::load_catalog` takes **already-resolved** inputs
(`registries: &[ResolvedRegistry]`, `access: &Arc<dyn OciAccess>`,
`badges: &BadgeContext`) — the caller does scope/registry/access resolution and
passes them in. The fetch core does the same. This is the option with the
smallest correct ripple that genuinely breaks the cycle (it does **not** create
a `command ↔ fetch` cycle, because the core takes only neutral lower-layer types
from `config` / `oci` / `install`).

**The reverse-edge helpers do NOT move.** `scope_resolution::*` and the
`registries_*` / `*_registry_*` helpers are genuinely command-layer
orchestration (they read config scopes, fold the global-config fallback tier,
apply flag/env precedence). Moving them would ripple through `search`, `status`,
`login`, `add`, `release`, `tui` and is unwarranted. `access_seam` likewise
stays in `command` (already consumed as `crate::command::access_seam` by other
front-ends). **Each caller resolves, then calls the pure core.**

**`ScopeToolArgs` does NOT move** — it stays in `mcp/tool_args.rs`. It is
flattened into *every* MCP tool's args (search/status/render/fetch); moving it
would ripple through all of them for zero benefit. The core never sees it.

**`command::grim` is not reachable from the core** (it lives in `command`).
The core inlines the identical wrap where it needs classification-correct
errors: `.map_err(|e| anyhow::Error::from(crate::error::Error::from(e)))?`.
`crate::error::Error` is the neutral top-level type (`From<AccessError>`,
`From<ClientTargetParseError>`, …) — no `command` dependency. (A bare `?` would
use the blanket `From<E> for anyhow::Error` and bypass classification — the
exact bug `grim()` exists to avoid — so the explicit wrap is load-bearing.)

### One new neutral bundling struct (in `src/fetch.rs`)

Mirrors `BadgeContext` — bundles the four co-resolved scope values threaded
through both core fns:

```rust
/// Resolved scope inputs for a fetch, computed once by the caller.
pub struct FetchScope {
    /// The ordered registry browse set (short-id + alias resolution).
    pub registries: Vec<crate::config::ResolvedRegistry>,
    /// The default registry for short-id expansion.
    pub short_id_default: String,
    /// The resolved scope kind (vendor mcp entries are scope-shaped).
    pub scope: crate::config::scope::ConfigScope,
    /// Warnings accumulated during scope resolution (e.g. degraded scope).
    pub warnings: Vec<String>,
}
```

`reference` / `vendor` / `path` stay bare borrowed params (as catalog keeps
`query` bare) — no extra request struct (YAGNI).

### Final core signatures (exact)

```rust
// src/fetch.rs
pub async fn fetch_artifact(
    scope: &FetchScope,
    access: &std::sync::Arc<dyn crate::oci::access::OciAccess>,
    reference: &str,
    max_layer_size: Option<u64>,
) -> anyhow::Result<FetchedArtifact>;

pub async fn fetch_with_limit(
    scope: &FetchScope,
    access: &std::sync::Arc<dyn crate::oci::access::OciAccess>,
    reference: &str,
    vendor: Option<&str>,
    path: Option<&str>,
    doc_limit: usize,
) -> anyhow::Result<FetchReport>;
```

Inside, `fetch_artifact` drops lines 137–167 (the scope block) and reads
`scope.registries` / `scope.short_id_default` / `scope.scope` / seeds
`warnings` from `scope.warnings.clone()`. `resolve_reference` is already neutral
(`crate::config::resolve_reference`). `fetch_with_limit` reads `args.vendor` →
`vendor`, `args.path` → `path`, and calls `fetch_artifact(scope, access,
reference, Some(FETCH_BLOB_SIZE_LIMIT))`.

### The one new command helper (single-sources the moved resolution)

To avoid duplicating the 137–167 block across three callers, extract it once —
in `command` (it depends on command helpers), returning the neutral type:

```rust
// src/command.rs  (or command/scope_resolution.rs — either; it uses command helpers)
pub fn resolve_fetch_scope(
    ctx: &crate::context::Context,
    global: bool,
    config: Option<&std::path::Path>,
    workspace: Option<&std::path::Path>,
) -> crate::fetch::FetchScope
```

Body = the current `mcp/fetch.rs:142–167` match, verbatim, returning
`FetchScope { registries, short_id_default, scope, warnings }`. This is
`command → fetch` (forward, fine); `fetch` never imports `command`.

### Post-extraction dependency graph (cycle gone)

- `command/fetch.rs → fetch` (core) ✅  — no longer imports `mcp`
- `api/fetch_report.rs → fetch` (core) ✅ — no longer imports `mcp`
- `mcp/fetch.rs → fetch` (core) + `mcp → command` (resolve_fetch_scope, access_seam) ✅
- `mcp/render.rs → fetch` (core) + `mcp → command` ✅
- `command → mcp`: **eliminated.**

`mcp → command` already exists pervasively (the server wraps `command::*::run`
seams — see `mcp.rs` module doc), so it is a pre-existing, acyclic direction.

### Caller rewrites (behavior-identical)

- **`command/fetch.rs::run`**: replace the `FetchToolArgs`/`ScopeToolArgs`
  construction + `crate::mcp::fetch::fetch_with_limit` call with:
  ```rust
  let scope = crate::command::resolve_fetch_scope(ctx, ctx.global(), ctx.config(), None);
  let access = crate::command::access_seam(ctx)?;
  let report = crate::fetch::fetch_with_limit(
      &scope, &access, &args.reference,
      args.vendor.as_deref(), args.path.as_deref(),
      crate::fetch::FETCH_BLOB_SIZE_LIMIT as usize,
  ).await?;
  ```
  Imports change from `crate::mcp::{fetch, tool_args}` to `crate::fetch::…`.
  `FetchCliReport` still wraps `crate::fetch::FetchReport`.
- **`mcp/fetch.rs::fetch`** (thin adapter kept for `server.rs`):
  ```rust
  pub async fn fetch(ctx: &Context, args: &FetchToolArgs) -> anyhow::Result<FetchReport> {
      let scope = crate::command::resolve_fetch_scope(
          ctx, args.scope.global(), args.scope.config.as_deref(), args.scope.workspace.as_deref());
      let access = crate::command::access_seam(ctx)?;
      crate::fetch::fetch_with_limit(
          &scope, &access, &args.reference,
          args.vendor.as_deref(), args.path.as_deref(),
          crate::fetch::FETCH_DOC_SIZE_LIMIT,
      ).await
  }
  ```
- **`mcp/render.rs::render`**: replace `super::fetch::fetch_artifact(ctx,
  &args.scope, &args.reference, None)` with:
  ```rust
  let scope = crate::command::resolve_fetch_scope(
      ctx, args.scope.global(), args.scope.config.as_deref(), args.scope.workspace.as_deref());
  let access = crate::command::access_seam(ctx)?;
  let fetched = crate::fetch::fetch_artifact(&scope, &access, &args.reference, None).await?;
  ```
- **`api/fetch_report.rs`**: `use crate::fetch::FetchReport;` (was
  `crate::mcp::fetch::FetchReport`).
- **`mcp/server.rs`**: unchanged (`crate::mcp::fetch::fetch`,
  `crate::mcp::render::render` still valid).

### Moved tests (call-site updates only — allowed under the refactor "import
paths after move" carve-out)

The `mcp/fetch.rs` `#[cfg(test)] mod tests` moves into `src/fetch.rs`. Its
`ctx_and_scope` helper currently builds a `ScopeToolArgs`; replace with a
`FetchScope` built via `crate::command::resolve_fetch_scope(&ctx, false, None,
Some(workspace))`. Every `fetch(&ctx, &args)` / `fetch_with_limit(&ctx, &args,
…)` / `fetch_artifact(&ctx, &args.scope, …)` call updates to the new signatures.
Assertions (messages, `fetch limit`, truncation marker, char-boundary, UTF-8
tail) are unchanged — behavior is preserved. `render.rs` tests reference
`crate::mcp::tool_args::ScopeToolArgs` for its own args and are otherwise
untouched.

---

## 3. CWE-770 cap threading

### Ground truth on oci-client

`registry_client.rs:329` calls `self.client.pull_blob(&reference,
digest_str.as_str(), &mut bytes)`. `pull_blob<T: AsyncWrite>(image, layer, out)`
(`external/rust-oci-client/src/client.rs:1403`) **already streams** the body
chunk-by-chunk (`response.bytes_stream()`, `out.write_all(&bytes).await?`) — the
only defect is the **sink**: an unbounded `&mut Vec<u8>` accumulates the whole
body before the digest is checked. So no different oci-client entry point is
needed: **swap the unbounded sink for a bounded one.**

(`pull_blob_stream` → `SizedStream` is the alternative, but consuming it needs
`futures_util::StreamExt`/`TryStreamExt`, which is **not** a direct grim
dependency — adding one to save a ~25-line sink violates "no new dep for a few
lines". The bounded `AsyncWrite` sink needs only `tokio` (already `features =
["full"]`) and keeps oci-client's internal digest verification intact.)

### Streaming-abort mechanism

Add a private sink in `registry_client.rs`:

```rust
/// An `AsyncWrite` sink that accumulates into a `Vec` up to `limit` bytes,
/// then refuses further writes so a registry serving more bytes than its
/// descriptor declared cannot stream an unbounded body into memory (CWE-770).
/// The abort is on ACTUAL bytes, independent of the descriptor's self-report.
struct CappedSink { buf: Vec<u8>, limit: u64, exceeded: bool }
```

`impl tokio::io::AsyncWrite for CappedSink`: `poll_write` — if
`self.buf.len() as u64 + data.len() as u64 > self.limit`, set `exceeded = true`
and return `Poll::Ready(Err(io::Error::other("blob exceeds size cap")))`;
otherwise `extend_from_slice` + `Ready(Ok(data.len()))`. `poll_flush` /
`poll_shutdown` → `Ready(Ok(()))`. (`CappedSink: Unpin`, so
`self.get_mut()` in `poll_write`.)

`fetch_blob` body change:
```rust
let mut sink = CappedSink { buf: Vec::new(), limit: max_bytes, exceeded: false };
match self.client.pull_blob(&reference, digest_str.as_str(), &mut sink).await {
    Ok(()) => {}
    Err(e) => {
        if sink.exceeded {
            return Err(AccessError::with_identifier(
                repo.clone(), AccessErrorKind::OversizeBlob { limit: max_bytes }));
        }
        return match lookup_failure(e) { None => Ok(None), Some(kind) =>
            Err(AccessError::with_identifier(repo.clone(), kind)) };
    }
}
let bytes = sink.buf;
// existing digest re-verify (defence in depth) stays unchanged, then Ok(Some(bytes)).
```
`&mut sink` is passed by ref (tokio's blanket `AsyncWrite for &mut T`), so
`sink.exceeded` / `sink.buf` are inspectable after the call. The digest is a
lie in the oversize case; abort happens before the re-hash, so no false
`DigestMismatch`.

### Trait signature change

`OciAccess::fetch_blob` (`oci/access.rs:86`) gains `max_bytes: u64`:
```rust
async fn fetch_blob(&self, repo: &Identifier, digest: &Digest, max_bytes: u64)
    -> Result<Option<Vec<u8>>, AccessError>;
```
Doc it: "aborts and errors (`OversizeBlob`) if the streamed body exceeds
`max_bytes`; callers pass the layer descriptor's declared `size` — the natural
ceiling, verified against policy caps before the call."

### New error variant + classification

`oci/access/error.rs` — add to `AccessErrorKind`:
```rust
/// The blob body exceeded the caller's byte ceiling mid-stream — the registry
/// served more bytes than the descriptor declared (CWE-770). Terminal.
#[error("blob exceeds the {limit}-byte size cap")]
OversizeBlob { limit: u64 },
```
`error.rs::classify_access` (166-ish) — add arm:
`AccessErrorKind::OversizeBlob { .. } => ExitCode::DataError` (65, same tier as
`DigestMismatch` — a lying descriptor is hostile/malformed data). The match is
non-wildcard, so the new variant compile-forces this arm.

### Cap value at every caller (uniform: the descriptor's declared size)

| Caller | Site | Pass |
|---|---|---|
| fetch core | `src/fetch.rs` `fetch_artifact` (moved from `mcp/fetch.rs:218`) | `layer.size` |
| installer | `install/installer.rs:607` `fetch_verified_layer` | `layer.size` (from `manifest.single_layer()`) |
| resolver | `resolve/resolver.rs:499` | `layer.size` |

Each call site already has the layer descriptor in scope and already runs its
pre-download descriptor gate (8 MiB / `MCP_LAYER_SIZE_LIMIT` /
`BUNDLE_LAYER_SIZE_LIMIT`). Passing `layer.size` means the streaming abort
enforces "the body may not exceed what the descriptor *claimed*"; combined with
the pre-gate (`layer.size ≤ policy cap`, where present) actual bytes are bounded
by the policy cap. The digest re-verify still enforces content correctness. The
pre-download gates are unchanged (they short-circuit the honest common case and
give kind-specific messages).

### Delegate + mocks to update (all `fetch_blob` impls/callers — full census)

**Production:**
- `oci/access.rs:86` — trait method signature.
- `oci/access/registry_client.rs:318` — real impl (the fix above).
- `oci/access/cached_access.rs:105` — add `max_bytes`; cache-hit path unchanged
  (a cached blob is already bounded/verified); miss delegates
  `self.inner.fetch_blob(repo, digest, max_bytes)` (line 117).
- `oci/access/memory_registry.rs:70` — add `_max_bytes: u64` (inert; test
  double stores exact bytes by digest).

**Call sites (add arg):** `fetch.rs` (moved), `installer.rs:607`,
`resolver.rs:499`. `cached_access.rs:117` (delegate).

**Test mocks (add `_max_bytes: u64`, inert):**
- `catalog/registry_catalog.rs:1175` (delegates → add arg to inner call at 1181), `:1520`
- `catalog/catalog_service.rs:307`
- `tui/update_check.rs:588`, `:876`
- `oci/access/cached_access.rs:212` (`CountingInner`)
- `install/installer.rs:916, :949, :984`
- `resolve/resolver.rs:786, :1299`

Existing `cached_access.rs` tests that call `access.fetch_blob(&id(), &digest)`
(lines 379/396/414) gain a cap argument (e.g. a large value like
`u64::MAX` or `payload.len() as u64`) — call-site-only edits, assertions
unchanged.

---

## 4. Commit ordering + file-by-file change list

Two commits, **Two-Hats separated** (structural refactor must not carry the
behavior change). Order: extraction first, fix second.

### Commit 1 — `refactor(fetch): extract neutral fetch core, break command↔mcp cycle`

Behavior-preserving. Acceptance tests (`test/tests/test_mcp_fetch.py`) and all
unrelated unit tests pass **unchanged**. Only the moved module's own tests get
call-site updates (per the refactor carve-out).

| File | Change |
|---|---|
| `src/fetch.rs` | **NEW.** Move `FETCH_BLOB_SIZE_LIMIT`, `FETCH_DOC_SIZE_LIMIT`, `TRUNCATION_MARKER`, `FetchedArtifact`, `FetchFileEntry`, `FetchReport`, `project_index`, `entry_content`, `cap_content`, and `fetch_artifact` / `fetch_with_limit` (new signatures §2). Add `FetchScope`. Move the `#[cfg(test)] mod tests` block; update its call sites. Inline the `crate::error::Error` error-wrap (no `command::grim`). |
| `src/main.rs` | Add `mod fetch;` between `mod error;` and `mod install;`. |
| `src/command.rs` | Add `resolve_fetch_scope` (the moved 142–167 block returning `FetchScope`). |
| `src/mcp/fetch.rs` | Shrink to the thin `fetch(ctx, &FetchToolArgs)` adapter. Delete moved items. Keep `FetchToolArgs`/`ScopeToolArgs` imports only as needed by the adapter. |
| `src/mcp/render.rs` | Rewrite the `fetch_artifact` call to resolve scope+access then call `crate::fetch::fetch_artifact` (§2). Change `use super::fetch::fetch_artifact` → `use crate::fetch::fetch_artifact`. |
| `src/command/fetch.rs` | Rewrite `run` to build `FetchScope`+access and call `crate::fetch::fetch_with_limit`; imports `crate::fetch::*` instead of `crate::mcp::{fetch, tool_args}`. |
| `src/api/fetch_report.rs` | `use crate::fetch::FetchReport;` (+ the test's `FetchReport` construction path). |
| `src/mcp/server.rs` | No change (verify only). |

Gate: `task rust:verify` green; `test/tests/test_mcp_fetch.py` green unchanged;
confirm no `crate::mcp::fetch` / `crate::mcp::tool_args` import remains in
`command/` or `api/` (the cycle is gone).

### Commit 2 — `fix(oci): bound blob download against a lying descriptor (CWE-770)`

Behavior-changing security fix.

| File | Change |
|---|---|
| `src/oci/access.rs` | Add `max_bytes: u64` to the `fetch_blob` trait method + doc. |
| `src/oci/access/error.rs` | Add `AccessErrorKind::OversizeBlob { limit: u64 }`. |
| `src/error.rs` | Add `OversizeBlob => ExitCode::DataError` arm in `classify_access`. |
| `src/oci/access/registry_client.rs` | Add `CappedSink` (`AsyncWrite`); rewrite `fetch_blob` to stream into it and map `exceeded` → `OversizeBlob`. Add the regression test (§5). |
| `src/oci/access/cached_access.rs` | Add `max_bytes` to impl; thread to inner (117); update the 3 in-module test call sites. |
| `src/oci/access/memory_registry.rs` | Add `_max_bytes: u64` (inert). |
| `src/fetch.rs` | `fetch_artifact` blob fetch passes `layer.size`. |
| `src/install/installer.rs` | `:607` pass `layer.size`. |
| `src/resolve/resolver.rs` | `:499` pass `layer.size`. |
| test mocks | Add `_max_bytes: u64` to every `fetch_blob` in `catalog/registry_catalog.rs` (2), `catalog/catalog_service.rs`, `tui/update_check.rs` (2), `install/installer.rs` (3), `resolve/resolver.rs` (2), `cached_access.rs::CountingInner`. |

Gate: `task rust:verify` + `task verify` green; new regression test green.

---

## 5. Risk / verification notes

### What guards the refactor (Commit 1)
- `mcp/fetch.rs` unit tests (moved into `src/fetch.rs`): canonical content +
  files listing, `--path` support file + missing-path error, bundle+vendor
  reject, oversize **descriptor** pre-gate (`fetch limit`), `fetch_with_limit`
  truncation control, char-boundary cap, UTF-8 partial-tail. These pin the
  content-shaping and gate behavior across the move.
- `mcp/render.rs` unit tests (skill tree, rule index+support dir, mcp/bundle
  reject) — unchanged; guard the render caller rewrite.
- `api/fetch_report.rs` tests (payload-plain, full JSON) — guard the re-export.
- `test/tests/test_mcp_fetch.py` — end-to-end MCP fetch; must pass unchanged
  (proves the extraction preserved the wire payload).
- `command/scope_resolution.rs` + `command.rs` registry-precedence tests —
  guard that `resolve_fetch_scope` reuses the same helpers (no precedence drift).

### New regression test for the CWE-770 fix (Commit 2)
**Lying-descriptor test in `registry_client.rs` `#[cfg(test)]`** (reuse the
existing throwaway-HTTP-server idiom — cf. `spawn_token_gated_registry`):
a server that answers the blob `GET /v2/<repo>/blobs/<digest>` with a body far
larger than the requested cap (e.g. serves ~4 MiB, or streams past the declared
size). Drive `RegistryClient::with_plain_http(&host).fetch_blob(repo, digest,
small_cap)` with `small_cap` well below the served size. Assert:
1. it returns `Err(AccessError { kind: AccessErrorKind::OversizeBlob { .. }, .. })`
   — **bounded abort, not a `DigestMismatch` and not OOM**;
2. the abort trips regardless of whether the digest matches (cap check precedes
   the re-hash);
3. `classify_access` maps it to `ExitCode::DataError` (unit-assert the mapping).

This is the failing-first regression proof: on the pre-fix `fetch_blob` the
unbounded `Vec` would swallow the whole oversized body (no error, memory
unbounded); the test fails without the `CappedSink`, passes with it.

### Residual notes
- Non-`registry_client` `OciAccess` impls (memory/mocks) do not enforce the cap
  — correct: the vulnerability is transport-level (a registry streaming more
  than declared), which only the real client can exhibit. The abort lives at the
  one seam that talks to the network.
- Install of skill/rule/agent still has **no policy cap** on `layer.size`
  (install parity) — out of scope. This fix closes only the
  body-exceeds-declared-size hole (`actual > layer.size`), which is the CWE-770
  vector named. An honest-but-huge declared size under no policy cap is a
  separate concern.
- `pull_blob`'s internal digest verification is bypassed on abort (we error
  first) and redundant on success (grim re-hashes at `registry_client.rs:341`) —
  no behavior change to the success path.
