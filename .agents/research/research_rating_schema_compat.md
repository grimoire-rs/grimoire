# Research: Schema Compatibility for the Rating Data Surfaces

<!--
Owner: hex-architect run 2026-08-17 (Research phase, data model / compatibility axis)
Handoff to: architect (ADR), /hex-plan
Extends: research_rating_backends.md (forge-as-database decision — not restated here)
-->

## Metadata

**Date:** 2026-08-17
**Domain:** packaging / data-model / security
**Triggered by:** [#82](https://github.com/grimoire-rs/grimoire/issues/82) — Ratings for catalog artifacts
**Expires:** 2027-02-17 (re-verify with `research_rating_backends.md`)

## Direct Answer

Grimoire already has a stronger compatibility invariant than any ecosystem surveyed here:
Principle 9 bans breaking changes outright, forever, not just "until the next major." That
changes the calculus versus npm/crates.io/OSV, which all *permit* an eventual major bump.
Given that, the right shape is: a **monotonic integer `schema_version`** on `ratings.json`
(crates.io's `v` field, not OSV's semver — semver communicates "minor vs. major," which
Grimoire has pre-committed to never needing), an **internally-tagged `provider.kind`
enum with a unit `#[serde(other)]` fallback**, and **`#[serde(default)]` + `Option<T>` on
every new field, never `deny_unknown_fields`, anywhere in the chain** — including on the
`CatalogEntry.rating` consumer side, which is the part of this design most exposed to a
silent, unreviewed compatibility break because nothing forces a downstream JSON consumer
(the VS Code extension, an MCP client) to be lenient. The cheapest CI check that would
catch a real break is **fixture-based**: a handful of `ratings.json` files with deliberately
injected unknown fields and an unknown `provider.kind`, deserialized by the real Rust
structs in a unit test — not a JSON-Schema-only check, which validates shape but not what
the parser actually does with the extra bytes.

## Q1 — Schema versioning for static published artifacts

| System | Mechanism | Verdict |
|---|---|---|
| **crates.io sparse index** | Per-line `v: <u32>` field, default `1` if absent. `v2` added `features2` as a *separate* field rather than changing `features`'s meaning, specifically because pre-1.19 Cargo couldn't parse the new feature syntax even with a lockfile present. Cargo ≥1.51 **ignores index lines with a `v` it doesn't recognize**; Cargo <1.51 ignores the field entirely and may misinterpret the entry. [Cargo Book](https://doc.rust-lang.org/cargo/reference/registry-index.html) | **Closest structural precedent.** A dumb integer, checked with "ignore what's higher," no semver ceremony. |
| **OSV schema** | `schema_version` as a full SemVer string, default `1.0.0` if absent. Explicit consumer contract: *"a client that knows how to read version 1.2.0 can process data identifying as schema version 1.3.0 by ignoring any unexpected fields."* Major bump reserved for real breaks. [osv-schema spec](https://ossf.github.io/osv-schema/) | Best **documented consumer contract** of anything surveyed — the sentence above is exactly the sentence Grimoire's own docs should contain for `ratings.json`. |
| **PyPI Simple API (PEP 691/700)** | Content-negotiated media type `application/vnd.pypi.simple.v1+json` plus a `meta.api-version` field inside the body (PEP 700 requires clients ask for `≥1.1`). Two versioning signals doing overlapping jobs. [PEP 700](https://peps.python.org/pep-0700/) | Works, but the dual signal (media type *and* body field) is more machinery than a single static file needs — media-type negotiation exists to let *the same URL* serve old and new clients differently, which doesn't apply to a file fetched by path. |
| **npm packument** | No version field at all — instead two documents at two `Accept` headers (`application/vnd.npm.install-v1+json` abbreviated vs. full). Evolution happens by adding keys to the (still unversioned) full packument. Some private registries **return 406 for the vendor Accept header** ([bun#341](https://github.com/oven-sh/bun/issues/341)) — a live example of content-negotiation being *less* portable than a plain static file. | Confirms: for a file served by a CDN/static host (this is exactly `ratings.json`'s deployment shape), skip content negotiation — it needs a server smart enough to branch on `Accept`, which a GitHub Pages / static index does not have. |
| **Go module proxy** | No schema version at all on `@v/*.info`; the protocol itself is versioned (there is no v2 protocol to date). Additive-only by convention, undocumented as a rule. | "Never version" works only as long as the *shape* never needs new required intelligence to interpret correctly — fine for a 3-field JSON blob, riskier as the document grows. |
| **in-toto / SLSA attestations** | `predicateType` URI embeds the **major** version (`.../slsa-provenance/v1`); minor/patch bumps are additive and don't change the URI; consumers **must ignore unrecognized fields** unless a predicate spec says otherwise. [SLSA versioning](https://slsa.dev/spec/v1.0/provenance), [in-toto versioning](https://github.com/in-toto/attestation/blob/main/spec/versioning.md) | Same "ignore unless told otherwise" rule as OSV, expressed via a type URI instead of a field — heavier machinery than warranted here (no multi-tenant predicate registry to disambiguate). |
| **OCI referrers/image-index** | `mediaType` + optional `artifactType` (RFC 6838) identify shape; distribution-spec v1.1 added the whole referrers mechanism additively on top of v1.0, and registries that don't support it degrade to a documented fallback (tag scheme). [image-index spec](https://github.com/opencontainers/image-spec/blob/main/image-index.md) | Confirms the general pattern (add a capability, define an explicit fallback for old participants) but is solving a different problem (content-type dispatch across an API), not applicable to a single static JSON file. |

**What actually caused pain**: the crates.io `features2` split exists *because* an early
version tried to widen the meaning of an existing field (`features`) instead of adding a
new one, and pre-1.19 Cargo's parser choked on syntax it had never seen — even a client
that would have been happy to *ignore* the new syntax couldn't, because the old field's
values became unparseable, not just semantically different. That is the one mechanical
rule worth carrying into `ratings.json`: **never change what a field can contain; add a new
field instead**, exactly as Principle 9 already states in the abstract.

**Recommendation for `ratings.json`**: `schema_version: 1` (integer, not string, not
semver) at the top level. Document, verbatim in the style of OSV's sentence: *"A client
that understands schema_version N must accept and correctly process any document with
schema_version ≤ N, ignoring fields it does not recognize; a document with schema_version
> N may be skipped or degraded to 'no rating available' but must never be treated as a
parse error."* Semver is overkill here because Grimoire has no "minor vs. major" axis to
communicate — Principle 9 already forbids the major-bump case that semver exists to signal.

## Q2 — Sidecar vs. embedded

| Ecosystem | Split | Why | Failure mode documented |
|---|---|---|---|
| npm | `registry.npmjs.org/<pkg>` (packument) vs. `api.npmjs.org/downloads/point/...` (counts) — **two separate services**, two separate uptime domains. [npm downloads API](https://github.com/npm/registry/blob/main/docs/download-counts.md) | Download counts churn constantly; packument metadata (versions, dist-tags) is comparatively static and cacheable. Splitting lets each scale/cache independently. | Not publicly documented as broken, but the split exists precisely to avoid coupling a fast-changing signal's staleness to the slow-changing document's cache TTL. |
| crates.io / GitHub Advisory / OSV | Advisory data is a **separate database** (osv.dev) aggregated from ~24 independently-versioned upstream sources (GHSA, PyPI advisory DB, RustSec, Go vulndb, …), joined by ecosystem+package+version at query time, not embedded in any registry's own index. [OSV overview](https://oneuptime.com/blog/post/2026-07-23-osv-practical-guide/view) | Same reasoning: advisory data changes on its own disclosure timeline, independent of a package's publish timeline. | **Documented join-skew failures exist**: `npm audit`'s GHSA-backed proxy has produced version ranges that mark already-patched versions as vulnerable because the join between "affected range" and "actual patched release" drifted — a real instance of a derived signal going stale relative to the thing it's about. ([GitHub Advisory Database now powers npm audit](https://github.blog/security/supply-chain-security/github-advisory-database-now-powers-npm-audit/), [community discussion of false positives](https://github.com/orgs/community/discussions/151960)) |

The npm-audit staleness case is the concrete cautionary tale for `ratings.json`: a
sidecar's staleness is invisible to the reader unless the contract makes staleness
observable. This is exactly why the settled design already carries `generated_at` in the
top-level block — that field is not decoration, it's the mechanism that lets a client
detect "this sidecar is older than I'm comfortable trusting" the same way OSV/GHSA
consumers currently *can't*. Recommend the design doc state explicitly that `generated_at`
staleness is a client UX decision (e.g. "rating data is N days old"), not silently ignored.

No registry embeds a fast-changing popularity/quality signal directly in its
slow-changing package manifest; the sidecar split adopted for `ratings.json` matches every
precedent surveyed. The two-fetch latency cost (reading `all.json` and `ratings.json`
separately) is the one real trade-off, and it is the same trade-off npm and OSV both
accepted deliberately.

## Q3 — Additive-only evolution mechanics (serde + TypeScript)

**Rust/serde — the concrete trap and the fix.** `#[serde(deny_unknown_fields)]` is a
compatibility landmine on any struct that deserializes a document you don't fully control
the producer of: a documented real-world case is an AWS Lambda Rust runtime library that
used it on a *request* struct — the maintainers noted that if the platform team ever adds
a new request field (a change every API provider considers non-breaking), every deployed
function using the library breaks in production ([serde-rs/serde#2634 discussion
context](https://github.com/serde-rs/serde/issues/2634)). Rule: **never** put
`deny_unknown_fields` on anything that deserializes `ratings.json`, `index.config.json`'s
`ratings` block, or a `CatalogEntry`. Use `#[serde(default)]` (not bare `Option<T>` without
`default`) for genuinely new fields so absence and JSON `null` don't need separate
handling paths, and reserve `Option<T>` for fields that are semantically nullable *within*
a version, not for "might not exist yet."

**Discriminator typing.** For `provider.kind`, the idiomatic and *only* supported
mechanism for a forward-compatible catch-all is `#[serde(other)]`, and it has a hard
constraint worth flagging (see Hazards below): it is **only valid on a unit variant of an
internally-tagged or adjacently-tagged enum** — not externally tagged, not untagged, and
the fallback variant **cannot carry data** ([serde.rs variant attrs, confirmed by direct
fetch](https://serde.rs/variant-attrs.html)):

```rust
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Provider {
    Github { repo: String },
    Gitlab { project: String },
    #[serde(other)]
    Unknown, // any future kind lands here — no fields preserved
}
```

**TypeScript — the equivalent pattern.** The recommended shape for a client SDK is an
explicit sentinel variant with a **distinct literal**, not a bare `string` discriminant
(which collapses TypeScript's narrowing):

```ts
type Provider =
  | { kind: "github"; repo: string }
  | { kind: "gitlab"; project: string }
  | { kind: "UNKNOWN"; raw: unknown };
```

This keeps exhaustiveness checking (`switch` over `kind` still narrows correctly) while
guaranteeing an unrecognized value never throws. ([Speakeasy: forward-compatible open
unions](https://www.speakeasy.com/blog/open-unions-typescript-type-theory))

**The general hazard this all guards against**: a "backward-compatible" additive change
can still break a *strict* client, and the break is never the producer's fault — it's a
client that promoted absence-tolerance into a hard requirement (`additionalProperties:
false` schema validators, exhaustive `switch` with no default, generated models that
reject unknown keys). The fix is symmetric on both ends: producers stay additive-only
*and* every consumer is built to ignore what it doesn't recognize, and both halves need to
be true or the "additive-only" guarantee is fiction. ([Stackademic: your
backward-compatible change can still break every strict client — direct
fetch](https://blog.stackademic.com/your-backward-compatible-api-change-can-still-break-every-strict-client-d50f9a7648c8))

## Q4 — Absent is first-class

**Prior art for absence as a designed state**, beyond `ratings.json`'s own "0 votes ==
omitted" rule: RFC 7396 (JSON Merge Patch) makes the null-vs-absent distinction load-bearing
at the protocol level — `null` means "delete this," absent means "leave alone" — precisely
because conflating the two states causes real data loss in PATCH semantics. The general
rule of thumb converged on across API-design guidance: use `null` for "considered, no
value," omit the key for "not applicable / caller can assume a default." Whatever the
choice, it has to be **documented and applied uniformly**, because a schema with some
optional fields nulled and others omitted, with no stated rule, is the failure mode itself
(inconsistency, not either policy alone).

**Failure mode when absence gets hardened into an error**: this is the same strict-client
problem as Q3, just from the other direction — a client that starts out correctly treating
"entry missing from `ratings.json`" as "unrated" can regress the moment someone adds a
"assert every catalog entry has a rating" check, a non-null constraint in a downstream
database, or a UI component that NPEs on `undefined` instead of rendering an empty state.
No single canonical postmortem exists for *this exact* pattern, but it is the mechanism
behind essentially every "GHSA data looked stale/wrong" npm-audit complaint cited in Q2 —
once a consumer stops treating "no record found" as a legitimate answer and starts treating
it as a data quality bug, every legitimately-absent record reads as broken.

**Recommendation**: state the absence rule at three levels explicitly in the design doc
(not just implicitly via "omitted == 0"): (1) `ratings.json` itself absent from the index →
whole index is unrated, not an error; (2) an artifact ref absent from `entries` → that
artifact is unrated; (3) `rating` absent from `CatalogEntry` in `grim ... --format json` →
unrated, and this must be `#[serde(default)]`/`Option` on the Rust side, never a field
consumers are allowed to assume is present. All three need a fixture-backed test (Q6) or
the “absent is first-class” claim is just a comment in a doc.

## Q5 — Deriving state rather than storing it (bot marker precedent)

This is the strongest-precedent question of the six — bots that own GitHub objects at
scale have solved exactly this problem for a decade, and the settled design's
`<!-- grim-ref: <ref> -->`-in-body marker is structurally identical to what they do.

- **giscus** (the design's own named precedent) searches GitHub Discussions by a chosen
  mapping key (pathname, title, URL) via the Discussions **search API**, and creates a new
  discussion on first reaction if no match is found. The documented failure mode is
  **duplicate discussion creation**: GitHub's search API does fuzzy/whitespace-insensitive
  matching, so near-identical mapping keys collide or fail to match, producing duplicates
  even with strict mode nominally on ([giscus#738](https://github.com/giscus/giscus/issues/738)).
  giscus's fix, shipped 2022-07-23, was to **hash** the mapping key and match on the hash
  embedded in the discussion body, rather than trusting title-string search directly — the
  same idea as the `grim-ref` marker, but with a fix already proven necessary in production.
- **Renovate** finds its own branches/PRs by matching **both** branch name *and* PR title,
  and only "recycles" (rebases in place rather than opening a new PR) when **every commit
  on the branch is bot-owned** — an explicit anti-hijack check, because a branch/PR that
  merely matches the naming convention but was touched by a human is not safe to silently
  overwrite. ([Renovate docs — pull requests](https://docs.renovatebot.com/key-concepts/pull-requests/))
- **Dependabot** identifies its own PRs by **author** (`dependabot[bot]`) plus a
  `dependencies` label, not by branch name alone — branch name is customizable per-repo and
  therefore not trustworthy as the sole identity signal.
- **release-please** embeds an explicit `x-release-please-version` marker *inside a source
  file comment* (not just the PR/issue body) for the one case where the state to reconcile
  isn't the object itself but a value nested inside tracked content, and separately uses
  PR **labels** (`autorelease: pending` → `autorelease: tagged`) as a state machine on top
  of the marker, because the marker alone doesn't capture lifecycle state.

**Documented reconciliation pattern, generalized from all four**: identity match must be
**at least two signals in AND**, never one — (marker string) AND (author/owner-account),
or (marker) AND (content-hash), never bare title/branch matching alone. Every bot above
learned this the hard way (giscus's duplicate bug, Renovate's explicit "all commits bot-owned"
gate, Dependabot's author+label combination).

**Direct application to the rating-thread creation job**: the tally/creation job must (1)
search for an existing `grim-ref: <ref>` marker, (2) additionally verify the discussion/work
item was created by the bot account before treating it as reusable, and (3) not trust a
single unpaginated search result as proof-of-absence before creating a new thread — giscus's
bug was exactly "searched, found nothing, created a duplicate" under conditions the search
API didn't reliably distinguish. This is a **hazard on the settled design**, flagged below.

## Q6 — Testing a compatibility guarantee

| Approach | Catches | Cost | Verdict for this design |
|---|---|---|---|
| JSON Schema validation in CI (`ajv`, `check-jsonschema`) | Shape violations against a declared schema | Low — one schema file, one CI step | Necessary but **not sufficient** — validates that a fixture matches the schema, says nothing about what the actual Rust/TS parser does with unknown fields |
| Golden/fixture-based deserialization tests | Exactly what Q3/Q4 are worried about: does the real parser choke on unknown fields, unknown enum values, missing optional fields | Low — a handful of hand-written `.json` fixtures + a unit test per language consumer | **The one to actually build.** Directly exercises the compatibility contract instead of a proxy for it |
| Contract/consumer-driven testing (Pact-style) | Cross-service contract drift between independently deployed producer/consumer | Medium-high setup cost, assumes a live producer/consumer pair | Overkill — `ratings.json` has no live producer to contract-test against; it's a static file |
| Schema-diff tooling (`buf breaking` for protobuf, `oasdiff` for OpenAPI) | Structural breaking changes between two schema versions, mechanically enumerated (100+ categorized rules in oasdiff; four severity tiers in buf: FILE/PACKAGE/WIRE_JSON/WIRE) | Medium — needs a formal schema (protobuf/OpenAPI) as the source of truth, which `ratings.json` doesn't have unless one is written | Directionally right idea (diff old-schema against new-schema, fail CI on breaking categories) but **wrong tool family** — there's no JSON-Schema-native equivalent of `buf breaking`/`oasdiff` with comparable maturity; a hand-maintained JSON Schema plus fixture tests gets the same coverage cheaper for a document this small |
| Old-binary-against-new-data matrix (compile N-1 release, run it against today's fixtures) | The real end-to-end guarantee — an actual previously-shipped client against actual new data | Highest — needs a pinned old binary or crate snapshot kept around | Worth doing once `ratings.json` ships and a real N-1 exists; not needed to *start* — the fixture tests below are the cheap version of the same idea, using "the current parser, fed data one version ahead of what it declares" as a stand-in for "an old parser fed current data" |

**Cheapest setup that would genuinely catch an accidental break here**: commit 3-4
fixture files under something like `test/fixtures/ratings/` —
(a) a minimal valid v1 document, (b) the same document with extra unrecognized top-level
keys, (c) the same document with an unrecognized `provider.kind`, (d) a document with an
artifact ref *absent* from `entries`. Deserialize all four through the real Rust structs in
a unit test (not just a JSON-Schema validator) and assert: (a)/(b)/(c) all succeed and
extract the fields they should, (b)'s unknown keys don't error, (c)'s unknown provider
degrades to the `Unknown`/`UNKNOWN` sentinel rather than failing, (d) reads as "no rating,"
not a missing-key panic. This is strictly cheaper than schema-diff tooling and catches the
exact failure class every ecosystem surveyed above actually broke on (crates.io's
`features2` syntax choke, `deny_unknown_fields`'s Lambda-runtime trap, giscus's duplicate
creation) — none of which a shape-only JSON Schema check would have caught.

## Hazards flagged against the settled design

1. **`provider` enum data loss on unknown kind.** `#[serde(other)]` requires the fallback
   variant be a **unit** variant — it cannot carry data. If a future provider (e.g. a
   self-hosted rating service, per the seam this design explicitly leaves open) needs
   fields beyond `kind`, an old client silently drops all of them, keeping only the fact
   that *some* unknown provider exists. If any client-side feature is ever expected to
   read provider-specific fields generically (rather than just gate on `kind == "github" |
   "gitlab"` and treat everything else as "rating exists, provenance unknown"), the design
   needs `#[serde(flatten)] extra: serde_json::Map<String, Value>` alongside `kind` instead
   of a tagged enum with a bare `Unknown` variant, to avoid a silent, unrecoverable
   information loss on first contact with a third provider. Confirm which shape is
   intended before locking the schema — this is a one-way door once `ratings.json` ships.
2. **`schema_version` type undecided in the source research.** The Discover-phase artifact
   doesn't specify integer vs. string vs. semver. Recommend integer (crates.io style) over
   semver (OSV style) specifically *because* Principle 9 removes the "minor vs. major" axis
   semver exists to communicate — semver here would be signaling a distinction the project
   has pre-committed to never making.
3. **`CatalogEntry.rating` is the least-guarded surface in this whole design.** `ratings.json`
   and `index.config.json` are both produced and consumed inside Grimoire's own tooling,
   where `deny_unknown_fields` discipline is enforceable by code review. `grim --format
   json` output is consumed by **external, independently-versioned code** (the VS Code
   extension per `product-context.md`'s Related Repositories table, MCP clients, anyone
   scripting against the frozen CLI contract) — exactly the shape of client the Q3 "strict
   client" failure mode targets, and Grimoire has no way to audit or fix those consumers'
   parsing discipline after the fact. The ADR should say explicitly that adding `rating`
   (and any future optional field) to `CatalogEntry` is safe *for grim's own JSON output
   guarantee*, but is only safe *in practice* if downstream consumers were already built to
   ignore unknown JSON keys — which is a documentation/communication problem, not a schema
   problem, and worth a line in the CLI's stability docs.
4. **Bot-owned-thread reconciliation needs a second identity signal.** The design's
   `<!-- grim-ref: <ref> -->` marker alone repeats the exact shape of giscus's duplicate-
   creation bug (search, find nothing, create a duplicate) unless the creation job also
   checks thread authorship (bot account) before treating a match as authoritative, and
   doesn't trust a single unpaginated search as proof of absence. This is a process/job
   design point, not a schema point, but it directly threatens the "derived, not stored"
   invariant the schema depends on — a duplicated thread means two different `target`
   values could plausibly map to the same `ref`, which the opaque-handle contract has no
   way to detect or repair from the client side.

## Recommendation

Adopt, in order of how load-bearing each is:

1. `ratings.json`: top-level `schema_version: <u32>` (crates.io style, not semver), default
   `1` if ever absent, documented with the OSV-style consumer sentence quoted in Q1.
2. `provider`: internally-tagged enum on `kind` with `#[serde(other)]` unit fallback for
   Rust — **contingent on confirming no client needs provider-specific fields from an
   unrecognized provider** (Hazard 1); otherwise flatten to a map instead of a tagged enum.
3. Every new/optional field across all three surfaces: `#[serde(default)]`, never
   `deny_unknown_fields`, anywhere a document from a different version of Grimoire (or a
   different index) might be read — including explicitly documenting this expectation for
   `CatalogEntry.rating`'s external consumers (Hazard 3).
4. State the absence rule at all three levels explicitly in the design doc (file, entry,
   field), not just as an implementation detail of the omit-zero-votes choice.
5. Add a second identity signal (bot authorship, not just the marker string) to the
   thread-reconciliation job, and treat "search found nothing" as provisional, not proof.
6. Ship 3-4 fixture files (Q6) as the actual compatibility test — cheaper than schema-diff
   tooling, and it's the only approach on the table that exercises what the parser
   *does* with unrecognized data rather than merely what shape a document has.

## Sources

| Source | Type | Relevance |
|---|---|---|
| [Cargo Book — Registry Index](https://doc.rust-lang.org/cargo/reference/registry-index.html) | Docs (fetched) | `v` field, `features2` split, old-client ignore behavior |
| [OSV Schema spec](https://ossf.github.io/osv-schema/) | Docs (fetched) | `schema_version` semver + explicit "ignore unrecognized fields" consumer contract |
| [PEP 700](https://peps.python.org/pep-0700/) | PEP | Media-type + body-field dual versioning |
| [npm download-counts docs](https://github.com/npm/registry/blob/main/docs/download-counts.md) | Docs | Sidecar download-count API, separate from packument |
| [bun#341](https://github.com/oven-sh/bun/issues/341) | Issue | Vendor Accept-header 406 on private registries |
| [GitHub Advisory Database now powers npm audit](https://github.blog/security/supply-chain-security/github-advisory-database-now-powers-npm-audit/) | Blog | Sidecar advisory integration |
| [npm audit false-positive discussion](https://github.com/orgs/community/discussions/151960) | Discussion | Documented join-skew / staleness failure |
| [serde-rs/serde#2634](https://github.com/serde-rs/serde/issues/2634) | Issue | `deny_unknown_fields` forward-compat trap (Lambda runtime case) |
| [serde.rs variant attributes](https://serde.rs/variant-attrs.html) | Docs (fetched) | `#[serde(other)]` constraints — unit-only, internally/adjacently tagged only |
| [Speakeasy — forward-compatible open unions](https://www.speakeasy.com/blog/open-unions-typescript-type-theory) | Blog (fetched) | TS sentinel-variant pattern, exhaustiveness preserved |
| [Stackademic — backward-compatible change can still break strict clients](https://blog.stackademic.com/your-backward-compatible-api-change-can-still-break-every-strict-client-d50f9a7648c8) | Blog (fetched) | The general strict-client failure mode underlying Q3/Q4 |
| RFC 7396 (JSON Merge Patch) | RFC | null-vs-absent as protocol-level distinct states |
| [giscus#738](https://github.com/giscus/giscus/issues/738) | Issue | Duplicate discussion creation — search-then-create race/fuzziness |
| [giscus README](https://github.com/giscus/giscus/blob/main/README.md) | Docs | Mapping strategies, hash-based strict matching fix |
| [Renovate — pull requests](https://docs.renovatebot.com/key-concepts/pull-requests/) | Docs | Branch+title match, bot-owned-commits recycle gate |
| [release-please manifest docs](https://raw.githubusercontent.com/googleapis/release-please/main/docs/manifest-releaser.md) | Docs | `x-release-please-version` marker, label-based lifecycle state |
| [SLSA provenance versioning](https://slsa.dev/spec/v1.0/provenance) | Spec | `predicateType` URI major-version encoding |
| [in-toto attestation versioning](https://github.com/in-toto/attestation/blob/main/spec/versioning.md) | Spec | Predicate-level SemVer, ignore-unrecognized-fields rule |
| [OCI image-index spec](https://github.com/opencontainers/image-spec/blob/main/image-index.md) | Spec | `artifactType`/`mediaType` dispatch, v1.1 additive rollout |
| [buf breaking change detection](https://buf.build/docs/breaking/) | Docs (fetched) | Schema-diff CI tooling, severity categories |
| [oasdiff](https://github.com/oasdiff/oasdiff) | Repo | OpenAPI schema-diff CI tooling, breaking-change rule count |
