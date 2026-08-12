# ADR: Catalog freshness — two clocks, cheap revalidation, caller-declared staleness

## Metadata

**Status:** Proposed
**Date:** 2026-08-11
**Deciders:** Michael Herwig (maintainer)
**Tech Strategy Alignment:**
- [x] Decision follows Golden Path in `.claude/rules/product-tech-strategy.md`
      (Rust 2024 + Tokio; **no new dependency** — conditional GET is hand-rolled
      on the already-pinned `reqwest`, cancellation uses `tokio::task::AbortHandle`
      rather than adding `tokio-util`, and jitter is deliberately not implemented
      rather than pulling `rand`)
**Domain Tags:** infrastructure, integration, security, tui
**Supersedes:**
[`adr_multi_registry_mcp.md`](./adr_multi_registry_mcp.md) **§ 4's final
sentence only** — "A long-lived MCP process additionally invalidates its
in-memory copy by catalog-file `mtime`." That sentence describes an in-memory
cache that does not exist: `McpState` (`src/mcp/state.rs:18-35`) holds `ctx` and
`allow_writes` and nothing else, and `src/mcp/server.rs:63-81` calls
`command::search::run` fresh on every tool call.
[`adr_mcp_percall_scope_fetch_render.md`](./adr_mcp_percall_scope_fetch_render.md)
moved the server to per-call reads, which made the intended in-memory cache
**unnecessary rather than merely unbuilt** — it is retired, not deferred.
This ADR also **discharges** that ADR's 2026-08-11 amendment clause 2 (the
`spawn_catalog_refresh` re-arm precondition) — see D5; the amendment's clauses
1, 3 and 4 stand unchanged and remain binding on D5's implementation.

**Not reopened.** A prior low-tier architecture decision settled the *freshness
model*: **one whole-catalog timestamp** (`CatalogFile.built_at`), no
`built_at`/`fetched_at` split, and a focused-row refresh is an **in-memory
overlay that never writes through to the catalog cache**. Rationale:
`design_freshness_model.md` + `review_freshness_model.md` (the third instance of
a shipped pattern — `grim status --check`'s `resolve_update_availability` and the
TUI's `spawn_row_checks` both already refresh per-entry metadata into memory and
never persist it). Every decision below honours it. Where D4 adds a persisted
`CatalogEntry.digest`, D4 states explicitly why that is not a contradiction.

**Research artifacts:**
[`research_catalog_revalidation_http.md`](../research/research_catalog_revalidation_http.md)
(technology axis — conditional GET, OCI `HEAD` digest, registry rate limits),
[`research_stale_while_revalidate_ux.md`](../research/research_stale_while_revalidate_ux.md)
(design-patterns axis — cargo/npm/apt/Homebrew precedent, RFC 5861, TUI refresh
models), and
[`research_git_remote_probe_security.md`](../research/research_git_remote_probe_security.md)
(security axis — empirically verified against git 2.54.0).

## Revision 2026-08-12 — the central decision is reversed by owner ruling

**O3 (two clocks + caller-declared `Freshness` policy) is withdrawn. The decision
is O1 + O5: keep the TTL a gate, make revalidation cheap, and make the TUI's
catalog load asynchronous for everyone.**

The adversarial review panel established that the weighted matrix below does not
select O3. Three of its eighteen cells were wrong, and they were the three
producing the 88-vs-116 gap:

- O1's *perceived browse latency* was scored 2/5 on the premise that it "still
  blocks". O1 includes D4, after which an index block is **one conditional round
  trip** — and the index is the default source, while the OCI `_catalog` walk is
  gated on the three largest registries by the crate's own
  `CATALOG_GATED_REGISTRIES`. Honest score: 4.
- O1's *Principle 9 risk* was scored 4/5 with no stated reason, when O1 touches
  no shipped signature and O3 adds a 9th parameter to `load_catalog`, a public
  enum, and a new arm to `coordinate`'s serve/rebuild ladder. Honest score: 5.
- O3's *network cost* was scored above O1's, but O3 makes strictly **more**
  network calls — it additionally ships the per-row fetches of D6. Honest: tie.

Re-derived with only those corrections: **O1 116, O3 112.**

**The sole stated reason to reject O1 was false.** "The TUI still freezes on
startup" — the freeze is `reload_into` awaited inline on the event-loop thread
(`src/tui/app.rs:1012-1076`). That is structural, fixed by D5's seam migration,
and orthogonal to `Freshness`. The policy enum removes roughly one round trip
from an already-async load; it unfreezes nothing. The plan's own S-004 (`r` stays
responsive) is delivered entirely by the seam migration, because D3 makes `force`
bypass the stale-serve arm regardless.

**O5, never weighed and now adopted.** Keep the TTL a gate; paint the TUI frame
first, spawn `load_catalog`, merge on arrival through the `CheckMsg` machinery
D5 already builds. A fresh cache returns in ~1 ms with no network; a stale one
returns after one 304. The UI is responsive throughout in both cases. Cost
against O3: rows appear ~1 RTT later instead of instantly. Buys: **`Freshness`,
`MAX_STALE_MULTIPLE`, the ceiling boundary tests, the D2 call-site pinning tests,
and the D3 `CatalogRequest` collapse are all deleted** — the 9th parameter that
motivated that refactor no longer exists.

### What this revision changes

| Section | Status after this revision |
|---|---|
| Considered Options matrix | **Superseded** — retained below as the reasoning trail; its scores are corrected here, not there |
| D1 (two clocks, ceiling) | **Superseded in part.** The per-source-kind floor survives; `MAX_STALE_MULTIPLE` and the ceiling are withdrawn. The floor is now **validator-gated** — see below |
| D2 (`Freshness` opt-out) | **Withdrawn entirely.** The TTL stays a gate for every caller, so there is nothing to opt out of |
| D3 (`CatalogRequest` collapse) | **Withdrawn.** No 9th parameter; the `#[allow(clippy::too_many_arguments)]` and its deferral reason stand unchanged |
| D4 (three additive fields, revalidation mechanics) | **Stands**, with two corrections below |
| D5 (seam migration) | **Stands, and is promoted** — it is now the primary delivery mechanism for the reported defect, not a supporting change |
| D6 (cancel-on-move focused row) | **Stands** |
| D7 (git leg) | **Stands**, with one correction below |
| D8 (migration/rollout) | **Stands**, minus the withdrawn pieces |

### Three corrections folded in from the review panel

1. **The index floor is validator-gated.** Dropping the index floor from 3600 to
   300 is a **12× revalidation-rate increase against every index host**,
   including self-hosted ones. It is only affordable if a revalidation is cheap,
   and whether a given host emits `ETag`/`Last-Modified` is not knowable in
   advance — no spot-check against one domain answers it for third parties.
   Therefore: `validator.is_some()` selects the window. A source with a stored
   validator uses `INDEX_TTL_SECONDS = 300`; a source without one keeps 3600 and
   its revalidation stays an unconditional GET at the old cadence. One predicate,
   no new machinery, and it degrades safely for hosts grim has never seen.
2. **A failed rebuild must not discard a usable cache.** Verified:
   `registry_catalog.rs:471-478` and `:508-515` return `Err` on build failure,
   dropping the catalog `coordinate` had already read and proved parseable, and
   `catalog_service.rs:311-315` then degrades that source to an empty group. A
   populated cache plus a transient network blip yields an empty browse — the
   exact apt `Valid-Until` failure shape this ADR cites to disqualify O1. Both
   error arms fall back to the already-read cache.
3. **The `built_at` restamp must restamp entries too.** `CatalogEntry.fetched_at`
   is a required, always-written field (`registry_catalog.rs:196`), so restamping
   only `built_at` on a 304 diverges the two on disk — which is precisely the
   `built_at`/`fetched_at` split the prior low-tier decision forbade. D4's claim
   that "`fetched_at` stays unwired and reserved" is false in consequence. The
   304 / tip-match path restamps every entry in the same rewrite (one map
   iteration), keeping the two clocks identical on disk.

### One correction to D7

`GIT_ALLOW_PROTOCOL` must include **`file`**. D7's allowlist as drafted omitted
it, which would have broken bare local `.git` index paths — the exact case C-019
and D7 both explicitly promise to keep working. Owner ruling: keep them working.
The allowlist is `file:http:https:ssh:git`.

## Context

Every browse read in `grim` is gated by one clock. `Catalog::coordinate`
(`src/catalog/registry_catalog.rs:527-587`) reads the per-registry cache file,
and if `built_at` is older than `CATALOG_TTL_SECONDS = 3600`
(`registry_catalog.rs:39`) it takes the advisory lock and rebuilds
**synchronously, in front of the user**. A rebuild is not cheap:

- **HTTP index** — an unconditional `GET <base>/all.json`, whole body, every
  time (`src/catalog/index_source.rs:152-178`). No validator is sent or stored;
  a grep of the whole crate finds zero occurrences of `ETag`, `If-None-Match`,
  `Last-Modified`, or `304`.
- **Git index** — a fresh `git clone --depth 1` of the entire index tree,
  unconditionally, every time (`index_source.rs:186-231`). The clone pays for
  the ref advertisement *and then* a full packfile, whether or not the tip moved.
- **OCI `_catalog`** — a `list_tags` + `resolve_digest` + `fetch_manifest` walk,
  three round trips per repository, up to `MAX_CATALOG_REPOS`, at
  `CONCURRENCY = 16` (`registry_catalog.rs:764-796`).

In the TUI this rebuild happens on the event-loop thread: `reload_into`
(`src/tui/app.rs:1012-1076`) `.await`s `catalog_service::load_catalog` inline, so
the whole interface freezes for the duration of a multi-registry network fan-out
— on startup and on every `r`. Five `TuiAction` arms block this way; two of them
(`Refresh`, `LoadVersions`) are in scope here.

The shape of the defect is the one apt is the textbook example of: a TTL used as
a **wall** rather than a floor. Past expiry, the read cannot proceed until the
network answers. Cargo and npm both retired that shape once conditional
revalidation made freshness self-certifying; Homebrew kept a TTL but demoted it
to a cheap non-blocking floor (`HOMEBREW_AUTO_UPDATE_SECS`, 60s).

Three further facts constrain any fix:

1. **The cache is wedge-prone.** `CatalogFile` and `CatalogEntry` are both
   `#[serde(deny_unknown_fields)]` (`registry_catalog.rs:222`, `:132`), and
   `coordinate` calls `Catalog::load(&path, &key)?` as its **first statement**
   (`:538`) — before the offline branch, before the freshness branch, before
   `force` is ever consulted. A parse failure therefore propagates out and
   `load_catalog` degrades that source to an empty group with a warn
   (`catalog_service.rs:311-315`), **without deleting the file**. That source
   browses empty on every subsequent run, `--refresh` included. The doc comment
   at `registry_catalog.rs:176-178` claims the opposite ("an older grim rejects a
   cache a newer grim wrote and **rebuilds it** — an accepted downgrade") and is
   currently the stated licence for additive fields on `CatalogEntry`.
2. **Re-arming the background catalog refresh is gated.**
   `adr_multi_registry_mcp.md`'s 2026-08-11 amendment records that
   `UpdateChecker::spawn_catalog_refresh` (`src/tui/update_check.rs:255`) emits a
   `Catalog`-shaped payload whose rows root in *locator* space while
   `registry_order` is in *root-key* space — a naive re-arm double-renders every
   registry, one copy as an empty `0/0` root sorted to `usize::MAX`.
3. **That same dead seam hardcodes `offline = false`** at its
   `load_or_refresh_coordinated` call (`update_check.rs:268`). It is inert today
   because nothing calls it. Re-arming it as written converts an inert hardcode
   into a live `--offline` bypass.

## Decision Drivers

- **A browse read must not block on the network when a usable answer is already
  on disk.** This is the whole point of the exercise.
- **`grim status --check`'s warnings must not get staler.** `deprecated` /
  `replaced_by` drive user-facing warnings and come from the catalog; the
  `CatalogScope::Complete` load (`src/command/status.rs:387-396`) exists
  precisely so a browse filter cannot hide one.
- **Principle 9 is a hard gate.** Released surfaces are frozen; schema evolution
  is additive-only. The JSON reports of `search` and `status` must be
  byte-identical after this change on identical inputs.
- **`--offline` must stay airtight.** The transport explorer verified the gate is
  sound through `load_catalog`; the one seam it could not clear is the one D5
  re-arms.
- **Rate-limit exposure is a concurrency problem, not a count problem.** Docker
  Hub exempts `HEAD` from its pull quota outright; GHCR publishes no cap. The
  reported 429s in both come from *parallel burst*, not sequential volume.
- **The git subprocess is a real trust boundary.** `[[registries]].index` is
  user-authored config, but it is the lowest-trust input that reaches a
  subprocess argv anywhere in the crate.
- **Boring technology, minimum frozen surface.** No new dependency, no new config
  key, no new CLI flag, no cache-format version bump.

## Industry Context & Research

**Key insight: the two research axes only appear to conflict, because they are
talking about two different clocks.** The SWR axis says demote the TTL from a
gate to a floor so the number stops mattering. The revalidation axis says keep
the OCI window near an hour while index sources go to minutes, because Docker Hub
and GHCR 429 under burst fan-out. Both are right, about different things:

- The **serve** decision (may I answer this read from cache?) should not consult
  a TTL at all in the common case. That is the SWR axis's finding, and RFC 5861
  is its formal shape — including its insistence that the staleness window be
  **explicit and finite**, of the same order of magnitude as the base freshness
  window, not 10–100× it.
- The **revalidate** decision (may I go to the network again yet?) must consult a
  clock, and that clock's right value depends entirely on what one revalidation
  costs. For an index source it is **one** round trip (a 304, or an
  `ls-remote` ref advertisement under 1 KB). For an OCI `_catalog` source it is a
  **fan-out over N repositories**. Those are not the same cost and cannot share a
  number.

Four further findings shape specific decisions:

1. **HEAD + `Docker-Content-Digest` is mandatory in the current
   distribution-spec** and is what `crane`, `skopeo`, `regctl` and the in-tree
   `oci-client` all already depend on — verified present on Docker Hub, GHCR,
   GitLab, Harbor and ECR. The spec itself carries the "older registries may
   omit it" caveat, so the fallback (absent digest ⇒ do the full fetch) is
   required, not optional.
2. **`http-cache-reqwest` is at `1.0.0-alpha.7`** and solves full RFC 7234
   caching — `Vary`, cache-control, disk cache managers — to replace roughly
   twenty lines of glue. Rejected on both grounds.
3. **`git ls-remote` is structurally safer than `git clone`** (no working tree,
   no submodule processing, no hooks — the entire 2024–2025 git CVE run is
   `clone`-side), **but is more dangerous on one specific axis**, verified
   empirically: with zero positionals left after option parsing it falls back to
   the *ambient* repository's configured remote, so a single config token shaped
   `--upload-pack=<payload>` is a complete RCE when `current_dir` is any git
   working tree. `--` closes it; git additionally rejects a dash-leading
   positional even after `--`.
4. **An ambient `GIT_ALLOW_PROTOCOL=ext` env var overrides `-c
   protocol.ext.allow=never`** — verified. The announce path's shipped idiom
   (`src/catalog/index_announce.rs:626-627`) is real protection against a weakened
   ambient *config* and no protection at all against an inherited env var.

## Considered Options — the central decision

The central decision is **what governs whether a browse read blocks**. The git
transport (D7) is a sub-decision of it, treated separately below.

| Criterion | Weight | O1 cheap revalidation only | O2 TTL as floor, unbounded stale | **O3 two clocks + caller-declared policy** |
|---|---|---|---|---|
| Perceived browse latency | 5 | 2 — still blocks, just for less time | 5 | **5** |
| Correctness of `--check` warnings | 5 | 5 — unchanged | 1 — warnings age without bound | **5** |
| Principle 9 / freeze risk | 5 | 4 | 2 — silently changes every consumer's freshness | **5** |
| Network cost / 429 exposure | 4 | 4 | 3 — a floor alone still permits an OCI fan-out at index cadence | **5** |
| Implementation + review surface | 3 | 5 | 4 | **3** |
| Reversibility | 3 | 5 | 3 | **4** |
| **Weighted total (max 125)** | | 88 | 76 | **116** |

### Option 1 — cheap revalidation only; the TTL stays a blocking gate

Ship steps 4 and 5, skip stale-while-revalidate. Every read still blocks on TTL
expiry, but the block is a 304 or a HEAD sweep instead of a full rebuild.

| Pros | Cons |
|---|---|
| Smallest diff; no new policy concept | The TUI still freezes on startup, which is the reported defect |
| Zero behavioural change for CLI consumers | An OCI source's block is still an N-wide fan-out, only cheaper per unit |
| Nothing to reverse | Leaves apt's `Valid-Until` failure shape intact: a degraded network still walls the read |

### Option 2 — demote the TTL to a floor for everyone; serve stale without bound

Every caller serves cache instantly regardless of age and kicks a background
revalidation. The number stops mattering, everywhere.

| Pros | Cons |
|---|---|
| Simplest mental model; one rule | RFC 5861's own prescription is violated: an SWR window without a finite bound is not SWR, it is "never expire" |
| Best-case latency for every front-end | A one-shot CLI has no owner for the background half — `grim status` exits before its revalidation lands, so it is pure waste |
| Matches cargo/npm's direction | `--check`'s deprecation warnings age without a ceiling; that is a silent correctness regression on a user-facing warning |
| | A single floor for both source kinds either over-polls OCI or under-polls indexes |

### Option 3 — two clocks, and stale-serving is a caller-declared policy (**chosen**)

Split the one clock in two, per source kind, and make "may I serve stale" an
explicit parameter that defaults to today's behaviour:

- A **revalidation floor** per source kind — how long before a *background* sweep
  is permitted. Never blocks a read.
- A **staleness ceiling** — a finite multiple of the floor, past which the cache
  is not usable for a non-blocking serve and the read blocks exactly as it does
  today.
- A **`Freshness` policy** the caller passes. `Blocking` (default) is today's
  semantics byte-for-byte. `StaleWhileRevalidate` is reachable only from a
  front-end that can own the revalidation — in practice, only the TUI.

| Pros | Cons |
|---|---|
| The TUI opens instantly; every CLI/JSON consumer is unchanged | Three constants where there was one; a new policy parameter on a shipped seam |
| Staleness is finite and explicit, per RFC 5861 | Requires the params-struct collapse (D3) to avoid a 9-parameter function |
| The floor's per-kind split matches a real structural cost asymmetry, not a preference | The floor/ceiling split has to be explained in docs |
| The `Blocking` default is what makes Principle 9 compliance provable rather than argued | |

**Option 4 — per-entry freshness / catalog write-through** is not reopened. It
was settled by the prior low-tier decision and its adversarial review; see
*Not reopened* in the Metadata block.

## Decision

### D1 — TTL semantics: two clocks, per-source-kind floor, finite ceiling

`CATALOG_TTL_SECONDS` **keeps its name and its value of 3600** and is
reinterpreted as the **OCI revalidation floor**. Two constants join it in
`src/catalog/registry_catalog.rs`:

```rust
/// Revalidation floor for a package-index source (HTTP or git). One
/// conditional round trip, so it can be short.
pub const INDEX_TTL_SECONDS: i64 = 300;

/// Staleness ceiling as a multiple of the floor. Past `built_at + N × floor`
/// the cache is not usable for a non-blocking serve and the read blocks —
/// RFC 5861's finite `stale-while-revalidate` window, same order of
/// magnitude as the base window rather than 10–100× it.
pub const MAX_STALE_MULTIPLE: i64 = 3;

/// The freshness window for a source kind.
pub fn freshness_window(kind: SourceKind) -> i64 { … }
```

So: **index sources revalidate after 5 min and are unusable past 15 min; OCI
`_catalog` sources revalidate after 1 h and are unusable past 3 h.**

The floor is **per source kind**, and this is not a tuning preference. One index
revalidation is `O(1)` round trips — a 304 with no body, or an `ls-remote` ref
advertisement under 1 KB. One OCI revalidation is `O(N)` repositories. The two
numbers describe two different costs; a single number necessarily
over-polls one or under-serves the other.

`is_fresh_at` takes the window explicitly (`is_fresh_at(built_at, window, now)`)
and `freshness_window(kind)` maps kind to window. Both are pure and unit-tested;
the existing `fresh_within_ttl_stale_after` / `future_timestamp_is_stale` tests
(`registry_catalog.rs:1031-1047`) are written against the symbolic constant and
survive with a window argument threaded in. The future-timestamp clock-skew guard
(negative age ⇒ stale) applies to both clocks unchanged.

**Jitter is deliberately not implemented.** The revalidation research recommends
it against synchronised bursts, but grim's revalidation is triggered by a human
opening the TUI or running `search` — arrivals are already spread by human
behaviour, and the advisory lock already suppresses the within-host herd. There
is no `rand` in the dependency tree and adding one for this is disproportionate.
*Upgrade path, named:* add deterministic jitter the day grim grows a
daemon/cron/CI-scheduled refresh, or the day a registry operator reports
synchronised bursts.

**Offline is exempt from both clocks.** `--offline` serves whatever is cached at
any age, never locks, never reaches the network — the frozen contract at
`docs/src/package-index.md:52-54`. The ceiling never turns an offline read into a
failure.

**Docs consequence.** `docs/src/package-index.md:52-54` and
`docs/src/hosting-an-index.md:143-146` both state "the 1-hour TTL" for index
transports and become numerically wrong. They are **reworded and renumbered** in
the same change: index sources say 5 minutes, and both pages gain one sentence
naming the ceiling. The TTL number is not in `docs/src/stability.md`'s frozen
list — that list covers CLI surface, JSON reports, schemas, wire format, index
*locators*, MCP tool names, schema URLs and env vars — and "everything else that
is not exit codes or JSON" is explicitly unstable. This is a documentation
correction, not a contract break.

### D2 — `grim status --check` opts out of stale-serving, and so does every other one-shot

**`CatalogScope::Complete` does not opt out. Process lifetime does.**

Overloading `CatalogScope` with a freshness policy would be the wrong seam.
`CatalogScope` answers "does the browse filter apply here" — a *filtering*
question with a documented correctness reason
(`catalog_service.rs:25-33`; `status.rs:387-396`'s own comment: a browse filter
hiding a deprecated declared artifact "would be a silent correctness bug, not a
display change"). Freshness is orthogonal to it.

The real discriminator is that **stale-while-revalidate has no owner in a
process that is about to exit.** `grim status --check` runs, prints, and returns.
A background revalidation it kicked off is either aborted at exit (pure waste) or
awaited before exit (not background at all). The same is true of `grim search`
under `CatalogScope::Browse`, and of every MCP tool call, which is a fresh
`command::search::run` per call (`src/mcp/server.rs:63-81`).

So the opt-out is expressed as a caller-declared parameter, not a scope variant:

```rust
/// Whether this caller may be served a stale catalog without blocking.
pub enum Freshness {
    /// Today's semantics, exactly: a stale cache blocks on a rebuild.
    /// Every one-shot caller — `search`, `status --check`, every MCP tool.
    Blocking,
    /// Serve a cache that is past its floor but within the ceiling, without
    /// locking and without touching the network; the caller owns the
    /// revalidation. Reachable only from a front-end with a lifetime long
    /// enough to land it.
    StaleWhileRevalidate,
}
```

**What it costs, precisely:** `grim status --check` against a cache older than
one hour still pays a blocking rebuild — identical to today. It does **not** get
worse, and D4 makes that block substantially cheaper (an index source pays one
conditional GET or one `ls-remote`; an OCI source skips the two-round-trip
manifest read for every repository whose digest is unchanged). The deprecation
staleness window stays at exactly its current value rather than widening to the
ceiling — a second, independent reason to opt out that holds even for a
hypothetical long-lived `status`.

Two things make this durable rather than a convention:

- The opt-in is inverted. `Freshness::Blocking` is what `Default` yields and what
  every existing call site gets; a caller must *ask* for stale-serving.
- It is pinned by test, using the house pattern that already protects
  `TUI_CATALOG_SCOPE` (`app.rs:997-1003` — "flipping it would make the browse
  filter inert on the TUI while every test stayed green"). Unit tests assert that
  `command/search.rs`, `command/status.rs` and the MCP path construct
  `Freshness::Blocking`, so a future edit cannot silently opt a JSON-emitting
  command into stale-serving.

Note also that `--check`'s `update_available` signal is *already* immune: it is
re-resolved live per artifact by `resolve_update_availability`
(`status.rs:404-410`), independent of the catalog. Only `deprecated` /
`replaced_by` are catalog-sourced, and this decision keeps their window where it
is.

### D3 — the seam: a params struct, collapsed first, in its own commit

`load_catalog` takes 8 parameters and carries an `#[allow(clippy::too_many_arguments)]`
whose stated reason (`catalog_service.rs:253-255`) is that collapsing it "is
deliberately deferred — mixing a refactor of a shipped signature into a feature
diff violates the Two Hats Rule". A 9th parameter is not acceptable, and a
separate `load_catalog_swr` would duplicate the locator dedup, the `JoinSet`
fan-out, the deterministic re-sort, the query filter and the badge pass — the
exact duplication the seam exists to end.

**Decision: stale-while-revalidate is a new field on the existing seam, and ADR
D5's deferred params-struct collapse happens now — as its own `refactor:` commit,
landing before the feature commit, with tests passing unchanged.**

That is the Two Hats Rule applied, not violated: the rule's remedy for "I need to
refactor and add behaviour" is *commit the refactor first, then switch hats*, and
D5 recorded the collapse as deferred, not forbidden. The refactor touches exactly
three call sites, is mechanical, deletes the `#[allow]`, and makes the feature
diff smaller.

```rust
/// One browse request against the configured registry set.
pub struct CatalogRequest<'a> {
    pub paths: &'a GrimPaths,
    pub registries: &'a [ResolvedRegistry],
    pub query: &'a str,
    pub access: &'a Arc<dyn OciAccess>,
    pub badges: &'a BadgeContext<'a>,
    pub offline: bool,
    pub force: bool,
    pub scope: CatalogScope,
    pub freshness: Freshness,   // added by the feature commit, not the refactor
}

pub async fn load_catalog(req: CatalogRequest<'_>) -> Result<CatalogResults, CatalogError>;
```

`freshness` is threaded down to `Catalog::coordinate`, which is the only place
that can act on it: the non-blocking serve has to happen *before* the rebuild
decision, and `coordinate` owns that decision. `coordinate` gains
`(freshness, window)` and its serve/rebuild ladder becomes:

```
offline                                   → Serve(cached or empty)          [unchanged]
fresh (age < window) and not force        → Serve(cached), no lock          [unchanged]
SWR and age < window × MAX_STALE_MULTIPLE
        and not force                     → Serve(cached), no lock, mark stale   [new]
otherwise                                 → lock, double-check, rebuild     [unchanged]
```

**`force` always wins.** `force = true` bypasses the freshness gate *and* the
SWR arm, and additionally suppresses conditional revalidation (D4): `--refresh`
means an unconditional fetch. That is the honest reading of a documented
"forces a catalog rebuild", and it is the user's escape hatch if a CDN ever
serves a wrong 304 or a git ref lies.

**Interaction with `--offline`:** the offline arm stays first, so no combination
of `freshness`/`force` can reach the network in offline mode.

`CatalogGroup` gains no field: it already carries `built_at`
(`catalog_service.rs:149-154`, currently `#[allow(dead_code)]` with the note
"captured for a future 'last refreshed' display; no consumer yet"). This work is
that consumer — the allow is deleted, not extended. The TUI reads `built_at` to
decide whether to sweep, and surfaces staleness through the existing
`RegistryHealth` status-line machinery (`state.rs:229-242`,
`render.rs:824-840`) as one more clause beside `offline` / `truncated` /
`filtered`. TUI appearance is explicitly unstable per `stability.md`.

### D4 — on-disk format: three additive fields, and the wedge fix is a hard precondition

**Every field added, exhaustively:**

| Struct | Field | Type | Written by | Read by |
|---|---|---|---|---|
| `CatalogFile` | `validator` | `Option<IndexValidator>` where `IndexValidator { etag: Option<String>, last_modified: Option<String> }` | HTTP-index rebuild, from the 200 response headers | next HTTP-index revalidation, as `If-None-Match` / `If-Modified-Since` |
| `CatalogFile` | `git_tip` | `Option<String>` (40-hex SHA) | git-index rebuild, from the clone's resolved tip | next git-index revalidation, compared against the `ls-remote` probe |
| `CatalogEntry` | `digest` | `Option<String>` (`sha256:…`) | **full OCI rebuild only**, the representative tag's manifest digest | **next full OCI rebuild only**, as the HEAD-probe baseline |

All three are `#[serde(default, skip_serializing_if = "Option::is_none")]`, so an
existing cache file parses unchanged and a source that never populates one never
serializes it. `CatalogVersion` stays `V1`.

The two validators are one nested struct rather than two flat fields precisely so
they cannot be replaced independently: an `ETag` from response A paired with a
`Last-Modified` from response B is incoherent, and the struct makes that
unrepresentable.

**`CatalogEntry.digest` does not contradict the not-reopened freshness model.**
It is a *rebuild-to-rebuild* baseline: written by `Catalog::build`'s full walk,
read by the next full walk to decide "may I skip this repository's manifest
fetch". It is **never** sourced from a focused-row refresh, and a focused-row
refresh never writes it or any other byte of the catalog file. The focused-row
overlay stays exactly what the prior decision made it: in memory, per session,
discarded on exit.

**Exact downgrade behaviour today, and why the wedge fix must land first.**
`CatalogFile` and `CatalogEntry` are `deny_unknown_fields`, so an older grim
reading a file carrying any of these three fields gets `Err` from
`serde_json::from_slice` → `CatalogError::parse` (`registry_catalog.rs:345`) →
propagated by the `?` on `coordinate`'s **first statement** (`:538`) → caught by
`load_catalog`'s `JoinSet` task, which warns and returns `(idx, None)`
(`catalog_service.rs:311-315`), degrading that source to an empty group. Nothing
on that path deletes, truncates or rewrites the file, and `force` is never
reached. **That source browses empty forever, `--refresh` included**, with no
in-product recovery. The comment at `registry_catalog.rs:176-178` asserting a
rebuild is false, and it is currently the licence under which additive
`CatalogEntry` fields are waved through.

The fix, landing in wave 1 before any field is added, is two lines of behaviour
and one comment:

1. **`coordinate` treats a parse/version failure as a cold cache** for the
   rebuild decision — warn once naming the path, then proceed exactly as for an
   absent file (offline ⇒ empty, online ⇒ rebuild over it; the atomic write
   replaces the unparseable file). `Catalog::load` keeps returning `Err`, so the
   `unknown_version_rejected` test (`registry_catalog.rs:1129`) stays valid; only
   `coordinate`'s handling changes.
2. **Drop `deny_unknown_fields` from `CatalogFile` and `CatalogEntry`.** This is
   the project's own stated additive-field rule applied to the one format that
   was exempt from it: `docs/src/stability.md` requires that "a consumer of
   either format must ignore fields it does not recognize rather than error on
   them". No human authors the catalog cache, so `deny_unknown_fields` buys no
   typo detection there — it buys only a downgrade wedge. **This is scoped to the
   catalog cache alone**; `grimoire.toml`, `grimoire.lock`, `publish.toml` and
   the MCP descriptor keep theirs, where they catch real human typos.

Together these retire the whole class: after wave 1, an unknown field is ignored,
and a genuinely corrupt file rebuilds instead of wedging.

**Principle 9 compliance: additive-only, confirmed.** Three new optional fields
with `default`, no field removed, no field's type or meaning changed, no enum
literal retired, no layout move, no version bump. `--format json` output is
untouched — `SearchEntry`/`SearchReport`/`StatusReport` gain nothing, and the new
fields live only in the disposable cache.

One honest residual, recorded as a Constitution Deviation below: a **pre-fix
released binary** sharing a `$GRIM_HOME` with a post-change binary still wedges,
because it lacks both halves of the wave-1 fix. Recovery is one line —
`rm -rf "$GRIM_HOME/catalog"` — and it goes in `docs/src/upgrading.md`. This is a
pre-existing property of the shipped format, not something this ADR introduces,
but this ADR is what makes it reachable, so it is documented rather than
discovered.

**Rejected: a sidecar validator file** (`catalog/<hash>.probe.json`) that an
older binary would simply not read. It removes the pre-fix wedge, and it costs a
second version envelope, a second lock story, a second reaper, and a duplicated
key set for the ≤500 per-entry digests. The benefit is confined to a transient
window before 1.0 in which a user runs two grim versions against one
`$GRIM_HOME`; the same review already rejected a sidecar overlay for the
per-entry freshness question. Not worth it.

**Revalidation mechanics, per source kind:**

| Source | Probe | On "unchanged" | Fallback |
|---|---|---|---|
| HTTP index | `GET` with `If-None-Match` (else `If-Modified-Since`) | `304` ⇒ keep entries, restamp `built_at`, rewrite the file | response carries neither validator ⇒ today's unconditional GET, no regression |
| Git index | hardened `git ls-remote -- <url> HEAD` (D7) | tip equals `git_tip` ⇒ keep entries, restamp `built_at`, rewrite the file; **no clone** | probe fails for any reason ⇒ fall through to the hardened clone |
| OCI `_catalog` | per-repository `HEAD` manifest, compare `Docker-Content-Digest` against `CatalogEntry.digest` | digest matches ⇒ reuse the cached entry, skip `fetch_manifest` | digest header absent (spec's own "older registries" caveat) ⇒ full manifest fetch for that repo |

The OCI probe keeps the existing `CONCURRENCY = 16` bound. Per the research, the
observed 429s on Docker Hub and GHCR come from parallel burst, not sequential
count, and Docker Hub documents `HEAD` as exempt from the pull quota. The
repository *enumeration* (`_catalog`) is unchanged and still paid in full — only
the three-round-trip per-repository metadata read is gated.

**`built_at` means "last confirmed current", not "last fully rebuilt."** The
restamp on a 304 / tip-match is mandatory: without it `built_at` never advances
and every browse re-probes. This *upholds* the one-timestamp decision rather than
softening it — there is still exactly one whole-catalog timestamp, and it answers
the only question anything asks of it. The field's doc comment is corrected;
`fetched_at` stays unwired and reserved, as decided.

**Two pre-existing gaps in `fetch_http` close in the same diff**, because the
function is being rewritten and both are Block-class under `quality-security.md`:

- **No redirect policy** (`index_source.rs:160-166`) — reqwest's default follows
  up to 10. `fetch_http` sends no credentials, so the header-replay leak class
  that made `forge.rs::build_client` set `Policy::none()` (`:263-278`) does not
  transfer, and a legitimate index may redirect (custom-domain Pages, `http`→
  `https`). Set `Policy::limited(5)` with a comment stating why it differs from
  the forge client rather than copying it.
- **No body size cap** — `.bytes().await` is unbounded. Apply the `CappedSink`
  idiom already used on the OCI blob path (`src/fetch.rs`) with
  `INDEX_BODY_LIMIT = 32 MiB`, surfacing `CatalogError::index_fetch`.

Also: the stored `ETag` is only valid for the same URL and the same
`Accept-Encoding` (a gzip representation gets a different validator). grim sends
one client configuration, so this holds by construction; it is noted in the field
doc so a future header change does not silently break the comparison.

### D5 — the seam migration: one projection, two producers, offline from context

The double-render risk is not in the message type. It is in there being **two
projections**: `reload_into` projects `CatalogResults` into root-key space, while
`drain_catalog_ready` (`app.rs:931-950`) projects a bare `Catalog` through
`rows_from_catalog`, producing `RowSource::Unattributed` rows that root in
locator space. Changing the payload type without deleting the second projection
would leave the bug intact.

**Decision:**

1. **Message type.** `CheckMsg::CatalogReady { catalog: Box<Catalog>, generation }`
   becomes `CheckMsg::CatalogReady { results: Box<CatalogResults>, generation }`.
   The spawned task calls `catalog_service::load_catalog`, not
   `Catalog::load_or_refresh_coordinated`.
2. **Merge.** The pure tail of `reload_into` — everything after the `.await`,
   i.e. `project_group_rows` / `apply_catalog_results` (`app.rs:1051,1058`) — is
   the **single** projection, called by both the foreground initial load and the
   drain arm. `rows_from_catalog` loses its last caller on this path.
   `merge_catalog_rows` (`state.rs:544-630`) keeps its existing job unchanged:
   preserve `marked`, `pinned_version`, `expanded_bundles` and the selection
   **anchor by row key, never by index** (`selection_anchor`/`restore_selection`,
   `:553`/`:629`).
3. **The index-aligned vectors move with the rows.** `registry_order` and
   `registry_locators` are rewritten from the same `CatalogResults` in the same
   call, per `adr_multi_registry_mcp.md`'s amendment clause 4 — a key absent from
   `registry_order` sorts silently to `usize::MAX`, so filling one without the
   other is a diagnostic-free bug.
4. **Scope.** The migrated seam reads `TUI_CATALOG_SCOPE` (`app.rs:1003`), the
   same hoisted constant `reload_into` uses, and the existing test that pins it
   covers the new producer too.
5. **Offline propagates from context, in two independent places.** The hardcoded
   `offline = false` (`update_check.rs:268`) is deleted. `UpdateChecker` gains an
   `offline: bool` field set at construction from `ctx.offline` — the checker is
   already constructed unconditionally regardless of offline mode
   (`app.rs:262-271`), so this is a field, not a lifecycle change — and the spawn
   passes `self.offline` through. `arm_background_checks`'s early
   `if ctx.offline { return; }` (`app.rs:614-617`) **stays**. Two gates, because
   one of them is the gate that was inert for the wrong reason. Pinned by a
   `#[tokio::test]` with a recording `OciAccess` double asserting **zero**
   network calls when the checker is constructed offline.
6. **`force` drops from `true` to `false`.** The sweep only fires when the TUI's
   own floor check says the cache is past its floor, so `coordinate` will decide
   to rebuild anyway — but with `force = false` the advisory lock's double-check
   still suppresses a redundant walk when a peer process already rebuilt. The
   user-initiated `r` keeps `force = true`. This is strictly better than today's
   hardcode and costs nothing.
7. **The floor decision is a pure function in the TUI.** `should_revalidate(built_at,
   kind, now) -> bool`, mirroring the shipped `eligible_for_recheck`
   (`update_check.rs:122-124`) and `should_schedule` (`:195-200`) pattern:
   decision pure and headlessly unit-tested, impurity confined to the spawn
   helper.

Step 6 (`load_versions` off the event loop, `app.rs:1942-1965`) follows the same
machinery — a `CheckMsg::VersionsReady { repo, tags, generation }` with a
`Loading` placeholder in the picker, generation-discard on drain (the picker is
modal; nobody benefits from cancel-on-move there), and **today's offline
behaviour preserved exactly** — the move is off the loop, not a semantics change.

### D6 — cancellation: cancel-on-move for the focused row, generation-discard for the sweep

**Focused row: cancel-on-move (yazi's model). Bulk sweep: generation-stamped
discard (unchanged).** The asymmetry is the point — the sweep's wasted work is
invisible, while the focused-row fetch holds a slot on a *shared bounded*
semaphore for an answer nobody will look at.

**The critical finding is that this needs no redesign of either existing
primitive.** `tokio::task::JoinSet::spawn` returns an `AbortHandle`; aborting a
task drops its future, and with it every local the task owns. Both things that
must be released are exactly such locals:

- the RAII `InFlightGuard` (`update_check.rs:370-384`) is moved into the async
  block, so its `Drop` runs and frees the `(repo, generation)` slot;
- the `OwnedSemaphorePermit` is acquired *inside* the task
  (`acquire_owned()`, `:336`) and returns to the semaphore on drop.

So `abort()` composes with the shipped dedup and concurrency machinery for free.
No `CancellationToken`, no `tokio-util` dependency, no change to `InFlightGuard`.

Machinery: `UpdateChecker` gains one field, `focus: Option<(String, AbortHandle)>`
— the repo currently being focus-checked and its handle. Re-selecting the *same*
row is a no-op; selecting a different row aborts the old handle before spawning
the new one.

**Debounce as well as cancellation, and they do different jobs.**
`FOCUS_COALESCE = 250ms` with a pure `should_refresh_focus(last, now)` in the
shape of the shipped `should_schedule` (`SEARCH_COALESCE = 300ms`, `:51`). The
debounce prevents the *spawn* while an arrow key is held; cancellation handles
the case where the user moves after a fetch has already started. Research found
no TUI that does both, and no TUI that does either well — yazi cancels without
debouncing, lazygit/k9s/btop poll a flat clock and never scope to the cursor.
250ms is the typeahead band, which is the closest sourced analogue for
"bursty, self-interrupting, directional input".

**Cancellation safety.** The task does `list_tags` → `pick_latest_tag` /
`pick_highest_version` → `resolve_digest` → `try_send`. Aborting mid-request
drops a connection. The one write on that path is `CachedAccess::list_tags`'s
write-through to `TagCache` (`cached_access.rs:145-158`), which is an atomic
write — it either completes or never starts, and no rename is left torn either
way.

**Known, accepted race.** `abort()` takes effect at the task's next await point,
so the `(repo, generation)` slot may free a tick after the abort. A re-selection
back to the same row inside that window could be suppressed by the dedup set. The
250ms debounce makes the window practically unreachable, and a suppressed check
re-fires on the next selection settle. Documented, not engineered against.

**What the focused-row refresh actually learns, and how it merges.** It calls one
`list_tags` and derives the representative tag and the highest concrete semver
with the existing `pick_latest_tag` / `pick_highest_version` helpers
(`registry_catalog.rs:861,896`) — the same two the catalog build uses.
`update_availability.rs` gains `resolve_latest_metadata` returning
`{ tag, version, digest }`, and the shipped `resolve_latest_digest` is refactored
to delegate to it so `status --check`'s call is unchanged and nothing is
duplicated.

Results merge as:

- an existing `CheckMsg::RowOutdated` when the row was `eligible_for_recheck`
  and `outdated_from_resolve` says so — reusing the shipped path means the
  focused-row feature inherits the `mark_outdated_if_installed` invariant for
  free: **flip every row sharing the `repo`, never stop at the first match**
  (`state.rs:822-840`), the exact defect fixed in `1a6fd68`;
- a new `CheckMsg::RowMetadata { repo, latest_tag, version, generation }`,
  merged by a new `TuiState::apply_row_metadata` under the identical fan-out
  rule, which may write **only** `latest_tag` and `version` — both marked "safe
  to overwrite" in the row-field survey — and must never touch `state`,
  `pinned_version`, `source`, `repo`, `registry` or `repository`.

That constraint is also the anti-flicker guarantee: the overlay touches no
identity field and no sort key, so a landing result never re-sorts, never
re-filters and never moves the cursor.

### D7 — the git leg: hardened `ls-remote` probe, hardened conditional clone, plus a config-time shape gate

Four options were weighed; the security research verified two facts that decide
it, both empirically, against git 2.54.0.

| | **A — keep `clone --depth 1`, harden it** | **B — `ls-remote` probe + conditional clone, both hardened (chosen)** | **C — HTTP ref-advertisement via `reqwest`** | **D — config-time shape gate** |
|---|---|---|---|---|
| Bytes per unchanged check | full packfile of the index tree | < 1 KB | < 1 KB | n/a |
| CVE surface | `clone`'s working-tree/submodule/hook machinery — the entire 2024–25 git CVE run | none found against `ls-remote` in the 2022–25 window | none (no subprocess) | n/a |
| Unique hazard | none beyond `clone`'s | zero-positional fallback to the ambient repo's remote ⇒ single-token RCE without `--` | breaks private indexes: `reqwest` does not consult git credential helpers | n/a |
| Transports covered | all | all | smart-HTTP only | n/a |
| Verdict | rejected — pays a full transfer to learn nothing | **chosen** | rejected — functional regression on a documented use case, plus hand-rolled pkt-line parsing | **adopted alongside B** |

**Chosen: B, with D's validation gate, and A's hardening applied to the clone
that B retains.** The clone does not go away — it is what runs when the tip
actually moved, or when the probe fails for any reason.

**Exact hardened probe invocation** (verbatim from the security research, whose
every clause was verified against a live git):

```rust
/// Wall-clock cap for the tip probe — git has no built-in equivalent.
const LS_REMOTE_TIMEOUT: Duration = Duration::from_secs(15);

let mut cmd = tokio::process::Command::new("git");
cmd.arg("-c").arg("protocol.ext.allow=never")   // vs a weakened ambient *config*
   .arg("-c").arg("http.followRedirects=false") // pin the transport; no host hop
   .arg("ls-remote")
   .arg("--exit-code")
   .arg("--")                                   // the URL can never parse as a flag
   .arg(&url)
   .arg("HEAD")
   .current_dir(&index_git_parent)              // never the caller's own cwd
   .env("GIT_TERMINAL_PROMPT", "0")
   .env("GIT_ALLOW_PROTOCOL", "http:https:ssh:git") // beats an *inherited* env var
   .env("GIT_ASKPASS", "echo")
   .env_remove("SSH_ASKPASS")
   .env("GIT_HTTP_LOW_SPEED_LIMIT", "1000")
   .env("GIT_HTTP_LOW_SPEED_TIME", "10")
   .kill_on_drop(true);                         // tokio does NOT kill on drop

let output = tokio::time::timeout(LS_REMOTE_TIMEOUT, cmd.output()).await…;
```

Each clause earns its place, and three are not covered by the announce path's
existing idiom:

- **`--` is load-bearing, not stylistic.** `ls-remote` with zero positionals left
  after option parsing falls back to the ambient repository's configured remote,
  so a single `[[registries]].index` value of `--upload-pack=<payload>` is a
  complete RCE when `current_dir` is any git working tree. Verified. `--` closes
  it, and git additionally rejects a dash-leading positional even after `--`.
- **`GIT_ALLOW_PROTOCOL` on the child, not just `-c protocol.ext.allow=never`.**
  An ambient `GIT_ALLOW_PROTOCOL=ext` inherited from the parent shell or CI job
  — not attacker-controlled, just an unrelated tool's leftover — silently
  overrides the `-c` flag. Verified. Only setting the variable explicitly on the
  child closes it. **This gap exists in the shipped announce path too**
  (`index_announce.rs:626-627`); closing it there is a named follow-up, not
  silently in scope here.
- **`timeout` + `kill_on_drop(true)` together.** Git has no wall-clock timeout;
  `GIT_HTTP_LOW_SPEED_*` only bounds an HTTP transfer already in flight, not a
  DNS stall, a TCP black hole or an SSH handshake that never speaks. And a
  `.output()` future dropped by a losing `timeout()` race does **not** kill the
  child by default — without `kill_on_drop` a timed-out probe orphans a process
  still holding a connection.

**The retained clone gets the identical treatment**, which it has none of today:
`--` before the URL and destination, the same env set, `current_dir`,
`kill_on_drop(true)`, and a `tokio::time::timeout` (120 s — a clone moves real
bytes, unlike the probe). It additionally stops interpolating the raw `url` into
its error message (`index_source.rs:211-217`, a CWE-532 credential-leak path when
the locator embeds a token) and routes it through the existing
`normalize_remote_url` (`git_provenance.rs:196-218`), which already strips
userinfo and is already reused by `index_announce.rs::index_host` — Utility
Discipline, not a new helper.

**Config-time shape gate (D).** `classify_index`
(`src/config/registry_resolve.rs:52-64`) classifies on a `.git` suffix alone, so
`--upload-pack=…evil.git` classifies as `IndexGit` and reaches argv, and
`ext::sh -c '…'.git` does too. Both are rejected at validation
(`project_config.rs::validate_registries`, `ConfigErrorKind::RegistryInvalid`,
exit **78** at config load and **65** at `grim config set` / `registry add`,
matching the shipped codes):

- a locator whose first character is `-`;
- a locator whose scheme is `ext::` after stripping a `git+` prefix.

**Deliberately *not* rejected: bare local paths ending in `.git`.** They classify
as `IndexGit` today and can serve a real index (a monorepo or air-gapped setup),
so rejecting them would be a breaking change under the Principle 9 freeze. The
two shapes above cannot fetch a real index at all — an `ext::` locator "works"
only as an exec primitive — so rejecting them removes no working configuration.
That is what makes this gate additive-safe; see the Constitution Deviations table.

### D8 — migration and rollout

**Existing caches.** Parse unchanged: every new field is `Option` + `default`, and
after D4's wave-1 fix an unknown field is ignored rather than rejected. The first
read after upgrade finds no validator and pays exactly today's full fetch, once,
per source. Validators populate on that rebuild. **No migration step, no reaper,
no version bump** — `CatalogVersion` stays `V1` deliberately: the only thing a
bump would buy is a clean rejection by older binaries, which is precisely the
wedge being removed.

**Existing configs.** Untouched. No new key, no new flag, no new env var. The TTL
values stay compiled-in constants: a config knob would add a frozen surface at
1.0 for a number that, after this change, no longer gates a read. Rejected as
YAGNI, and named here so it is not re-proposed.

**Downgrade.** After wave 1, an older-but-post-fix grim reading a newer file
ignores the unknown fields and works. A **pre-fix released binary** sharing a
`$GRIM_HOME` wedges the affected source to an empty browse; recovery is
`rm -rf "$GRIM_HOME/catalog"`, documented in `docs/src/upgrading.md`.

**Wave order — the first wave is a hard precondition, not a preference.**

| Wave | Contents | Why here |
|---|---|---|
| **1** | Cache-wedge fix (`coordinate` treats parse/version failure as cold; drop `deny_unknown_fields` from the two cache structs; correct the false claim at `registry_catalog.rs:176-178`). Stale module doc at `catalog_service.rs:14-17`. Dead-code disposition. `load_catalog` → `CatalogRequest` collapse as its own `refactor:` commit, tests unchanged. | Nothing may add a field to `CatalogEntry` until the downgrade story is true. The refactor lands before the feature per Two Hats. |
| **2** | Step 4 — conditional GET, `ls-remote` probe, `HEAD`-digest gate, the three new fields; git subprocess hardening; `classify_index` gate; `fetch_http` redirect policy + body cap. | Makes every blocking rebuild cheap. Valuable on its own even if wave 3 never shipped. |
| **3** | Step 5 (two clocks, doc rewording) + step 3 (`Freshness` policy, SWR serve path in `coordinate`). | The policy is only worth having once revalidation is cheap. |
| **4** | Step 2 (seam migration off the event loop) + step 1 (focused-row refresh) + step 6 (background `load_versions`). | All three consume wave 3's policy and each other's machinery. |

**Test plan, by layer** (the acceptance suite cannot see freshness directly —
there is no lever but `--refresh` and `--offline`, and TTL expiry is untestable at
1 h):

- **Rust unit** — pure decision functions (`is_fresh_at` with an explicit window,
  `freshness_window`, `should_revalidate`, `should_refresh_focus`,
  `apply_row_metadata`'s fan-out); `coordinate`'s serve/rebuild ladder against a
  temp dir including the corrupt-cache-is-cold case (no test covers corrupt
  caches today); `#[tokio::test]`s against hand-rolled `OciAccess` doubles in the
  shipped `GatedAccess`/`VersionedAccess` style for cancellation, dedup-slot
  release on abort, and the offline zero-call assertion.
- **Acceptance (pytest)** — the `http_index` fixture is a real
  `ThreadingHTTPServer` and is extended with a `BaseHTTPRequestHandler` subclass
  that records `If-None-Match` and answers `304`, proving the conditional path
  end to end (`SimpleHTTPRequestHandler` cannot do this, which is why no such
  test exists). The git fixture already mutates a real repo
  (`test_git_index_refresh_picks_up_new_packages`); a sibling asserts an
  unchanged remote still serves the same rows. Plus one regression test per
  rejected locator shape from D7's config gate, asserting exit 78 / 65.
- **Explicitly not acceptance-tested** — the interactive TUI, per that suite's own
  module docstring; all of D5/D6 is covered headlessly in the pure modules.

**First-party catalog drift duty.** This work touches
`docs/src/package-index.md` and `src/command/**`, so `catalog/README.md`'s
"Keeping content honest" duty fires for `catalog/skills/grim-usage` and
`catalog/skills/grim-authoring`. `ai-config-authoring` is not implicated
(`clients.md` / `vendor-metadata.md` are untouched).

## Consequences

**Positive**

- The TUI opens on cached data immediately and never freezes on a network
  fan-out; the `RegistryHealth` line says when what you are looking at is stale.
- Every blocking rebuild that remains is dramatically cheaper: one 304 for an
  HTTP index, one sub-kilobyte ref advertisement for a git index, and a
  `HEAD`-gated walk that skips two round trips per unchanged repository for OCI.
- `grim search`, `grim status` and every MCP tool are behaviourally identical,
  byte for byte, on identical inputs.
- The catalog cache becomes forward-compatible for the first time: unknown fields
  are ignored and a corrupt file rebuilds instead of wedging. The shipped comment
  at `registry_catalog.rs:176-178` becomes true.
- Four shipped security gaps close in files the work already opens: the missing
  `--` and protocol allowlist on the index clone, the redirect policy and the
  unbounded body on the index HTTP fetch, plus the CWE-532 URL interpolation in
  the clone's error text.
- `spawn_catalog_refresh`'s inert `offline = false` is removed before it could
  become live, and `CatalogGroup.built_at`'s `#[allow(dead_code)]` is deleted
  rather than extended.

**Negative / risks**

- **A 304 is trusted.** A CDN or proxy serving a wrong `304` pins a stale
  catalog until the ceiling. Mitigated: `--refresh` suppresses conditional
  headers entirely (D3), so the escape hatch is a documented flag, not file
  surgery.
- **`ls-remote` sees the tip, not the content.** An unrelated commit to the index
  repo triggers a needless rebuild — a false positive, never a false negative,
  under fast-forward-only pushes to the tracked ref.
- **Three constants where there was one**, plus a policy parameter on a shipped
  seam. Mitigated by the pinning tests and by `Blocking` being the default.
- **A pre-fix binary still wedges** on a post-change `$GRIM_HOME`. Documented
  recovery; unavoidable without a sidecar that costs more than it saves.
- **The focused-row refresh extends `TagCache` write-through to `NotInstalled`
  rows**, which never triggered one before — one small file per repository the
  user actually dwelt on. Bounded by the 250 ms debounce; flagged below.
- **Wave 1 is a genuine serialisation point.** Waves 2–4 cannot start before it
  lands, which costs parallelism in execution.

**Reversibility.** Two-way at every wave. Wave 1 is strictly a correction. Wave 2
is additive fields plus hardening; reverting means the fields go unread (they
still parse, since `deny_unknown_fields` is gone). Waves 3–4 are a policy
parameter and TUI machinery; reverting is deleting a variant, a message and a
spawn helper, with no data migration and no user-visible contract touched. The
one direction that is not cheap to reverse is dropping `deny_unknown_fields` —
re-adding it would re-create the wedge, which is why it is scoped to the cache
and stated explicitly rather than done quietly.

## Constitution Check

Checked against the nine Core Principles in `AGENTS.md`.

| Principle | Status |
|---|---|
| 1 Understand First | Held — every load-bearing claim carries a `file:line`; the whole read/write surface was mapped before designing. |
| 2 Prove It Works | Held — test plan per layer in D8; the untestable-at-1h TTL is stated rather than papered over. |
| 3 Keep It Safe | Held, and net-positive — four shipped gaps close; every new subprocess clause is empirically verified. |
| 4 Keep It Simple | Held — no new dependency, no config knob, no jitter, no `CancellationToken`, no new checker struct; the focused-row feature reuses the shipped `RowOutdated` path. |
| 5 Don't Repeat Yourself | Held — one projection with two producers (D5), `resolve_latest_digest` delegates to the new seam, `normalize_remote_url` and `CappedSink` reused. |
| 6 Ship It | Held — feature branch, no push. |
| 7 Leave a Trail | Held — this ADR, plus the amendment text below. |
| 8 Learn and Adapt | Held — the `GIT_ALLOW_PROTOCOL` finding is recorded against the shipped announce path as a follow-up, not silently fixed. |
| 9 Preserve Compatibility | **Two deviations, both recorded below.** |

### Constitution Deviations

| # | Principle | Deviation | Justification | Mitigation |
|---|---|---|---|---|
| CD-1 | 9 | `classify_index` starts rejecting two locator shapes it accepts today (leading `-`, `ext::` prefix), at config-load 78 / `config set` 65. | Neither shape can fetch a real package index; an `ext::` locator functions only as a command-execution primitive, and a `-`-leading locator only as argv injection. Rejecting them removes no working configuration, so the freeze's purpose is served rather than breached. Bare local `.git` paths are deliberately **not** rejected for exactly this reason. | Regression test per rejected shape; the reason string names the accepted forms; release notes call it out. |
| CD-2 | 9 | A pre-fix released binary sharing `$GRIM_HOME` with a post-change binary wedges that source to an empty browse, and `--refresh` cannot clear it. | A pre-existing property of the shipped `deny_unknown_fields` cache format, not introduced here — but this ADR is the change that makes it reachable, so it is owned rather than discovered. A version bump would not help; a sidecar costs more than it saves (D4). | One-line recovery documented in `docs/src/upgrading.md`; the wave-1 fix means it never happens again after this release. |
| CD-3 | 9 (informational) | The documented index TTL changes from 1 hour to 5 minutes in two doc pages. | The TTL number is not in `docs/src/stability.md`'s frozen list, and "everything else that is not exit codes or JSON" is explicitly unstable. | Both pages reworded in the same change; the ceiling documented alongside. |

## Open questions

Three, all genuinely unresolvable in this session rather than deferred by choice.

1. `[NEEDS CLARIFICATION: live header check]` — no network access was available
   to run `curl -sSI https://index.grimoire.rs/all.json` and confirm that `ETag`
   and/or `Last-Modified` survive whatever CDN fronts the custom domain. The
   Fastly-backed GitHub Pages stack emits both by convention, and the fallback
   (neither validator present ⇒ today's unconditional GET) is safe either way —
   but **run the spot-check before wave 2 ships** and record the result.
2. `[NEEDS CLARIFICATION: OCI fan-out in the field]` — the 1 h OCI floor with a
   16-wide semaphore is derived from third-party reports of GHCR/Docker Hub 429s
   under parallel load, not from measurement against a real grim install browsing
   a large namespace. If field reports show 429s, the lever is the concurrency
   bound first and the floor second, in that order.
3. `[NEEDS CLARIFICATION: focused-row tag-cache churn]` — extending the focused
   refresh to `NotInstalled` rows writes one `TagCache` file per repository the
   user dwells on for ≥250 ms. Expected to be a handful per session, unmeasured.
   If it proves noisy, the cheap fix is to keep `eligible_for_recheck`'s gate for
   the *write-through* while still serving the in-memory overlay.

## Amendment to record in `adr_multi_registry_mcp.md`

Per repo convention a decision text that reality overtook gets a dated amendment
block, not an in-place edit (cf. `adr_registry_filter_match_candidate.md`
superseding `adr_registry_browse_filters.md` § D3). Add beneath § 4:

> **Amended 2026-08-11 (catalog freshness / revalidation — see
> [`adr_catalog_freshness_revalidation.md`](./adr_catalog_freshness_revalidation.md)).**
> The final sentence above — "A long-lived MCP process additionally invalidates
> its in-memory copy by catalog-file `mtime`" — is **retired, not deferred**.
> `McpState` (`src/mcp/state.rs:18-35`) holds `ctx` and `allow_writes` and no
> catalog at all, and `src/mcp/server.rs:63-81` calls `command::search::run`
> fresh on every tool call. `adr_mcp_percall_scope_fetch_render.md` moved the
> server to per-call reads, which made the intended in-memory cache
> *unnecessary* rather than merely unbuilt — there is nothing to invalidate.
> Separately, § 1's 2026-08-11 amendment clause 2 (re-arming
> `spawn_catalog_refresh` is gated on migrating the seam onto
> `catalog_service::load_catalog`) is **discharged** by that ADR's D5, which
> specifies the migration; clauses 1, 3 and 4 stand unchanged and remain binding
> on it.

## Implementation Plan

1. [ ] Wave 1 — wedge fix, `deny_unknown_fields` removal, comment corrections,
       dead-code disposition, `CatalogRequest` collapse (`refactor:` commit).
2. [ ] Wave 2 — revalidation for all three source kinds, the three additive
       fields, git subprocess hardening, `classify_index` gate, `fetch_http`
       redirect policy + body cap.
3. [ ] Wave 3 — two clocks, `Freshness` policy, SWR serve path, doc rewording of
       `package-index.md` and `hosting-an-index.md`.
4. [ ] Wave 4 — seam migration, focused-row refresh, background `load_versions`.
5. [ ] Add the ADR index row to `.claude/rules/arch-principles.md` (repo
       convention: every ADR carries one).
6. [ ] Add the amendment block above to `adr_multi_registry_mcp.md`.
7. [ ] Drift-review `catalog/skills/grim-usage` and `catalog/skills/grim-authoring`.
8. [ ] Document the pre-fix-binary recovery in `docs/src/upgrading.md`.
9. [ ] Follow-up, separate change: close the `GIT_ALLOW_PROTOCOL` gap on the
       shipped announce path (`index_announce.rs:626-627`).

## Validation

- [ ] `task verify` green; no new clippy allow introduced (one deleted).
- [ ] `grim search --format json` and `grim status --check --format json` output
      byte-identical before/after on identical inputs.
- [ ] Offline zero-call test passes for the re-armed background seam.
- [ ] Conditional-GET acceptance test observes `If-None-Match` and a `304`.
- [ ] Security review of the two git invocations against the verified argv/env
      list in D7 — checked against the *final* argv, per the GitPython
      incomplete-fix precedent, not "we added `--` somewhere".

## Links

- [`adr_multi_registry_mcp.md`](./adr_multi_registry_mcp.md) — the per-registry
  cache layout, `AdvisoryFileLock` and the shared `load_catalog` seam this builds on
- [`adr_mcp_percall_scope_fetch_render.md`](./adr_mcp_percall_scope_fetch_render.md)
  — per-call MCP reads, which retired the in-memory catalog cache
- [`adr_registry_browse_filters.md`](./adr_registry_browse_filters.md) — `CatalogScope`,
  and D5's deferred params-struct collapse discharged here
- [`adr_fetch_service_extraction.md`](./adr_fetch_service_extraction.md) — the
  `CappedSink` / `OversizeBlob` bounded-body idiom reused for the index fetch
- [`adr_announce_fork.md`](./adr_announce_fork.md) — the shipped git-subprocess
  hardening list this extends
- [`docs/src/stability.md`](../../docs/src/stability.md) — what is frozen at 1.0

---

## Changelog

| Date | Author | Change |
|------|--------|--------|
| 2026-08-11 | architect (`/hex-plan high`, Phase 4) | Initial decision — D1–D8 |
