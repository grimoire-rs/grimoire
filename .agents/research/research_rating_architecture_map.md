# Research: Architecture Map — Rating System Surfaces

<!--
Owner: hex-architect run 2026-08-17, Discover phase (architecture-explorer)
Handoff to: architect (ADR), /hex-plan
Companion to: research_rating_backends.md (settled design + forge APIs),
research_rating_schema_compat.md, research_rating_security.md,
research_rating_operability.md
-->

## Metadata

**Date:** 2026-08-17
**Domain:** cli / packaging
**Triggered by:** [#82](https://github.com/grimoire-rs/grimoire/issues/82)
**Expires:** 2026-11-17 (code map decays fast — re-verify line numbers)

## Direct Answer

Every seam the rating feature needs already exists in some form. The genuinely new
work is: a GraphQL payload/response layer (both repos), a `reconcile`/`tally` verb in
the indexer (no analog beyond `enrich`), and a runtime-fetch decision for the static
site, which today has none.

## Component map (grim)

| Area | Location | Seam |
|---|---|---|
| Index fetch | `src/catalog/index_source.rs:150-178` `fetch_http`, `:186-231` `fetch_git` | Plain unconditional GET of `<base>/all.json`; a `ratings.json` sibling fetch attaches here |
| Index → entry projection | `index_source.rs:79-122` `into_entry()`; `IndexPackage` all fields `#[serde(default)]` | Add `rating` to `IndexPackage`, forward in `into_entry()` — exact pattern `deprecated`/`replaced_by` use |
| Catalog entry type | `src/catalog/registry_catalog.rs:133-197` `CatalogEntry` | `rating: Option<RatingSummary>` with `#[serde(default, skip_serializing_if)]` |
| Cache envelope | `registry_catalog.rs:220-241` `CatalogFile`, `#[serde(deny_unknown_fields)]`; path `src/store/paths.rs:83,88` | Per-registry cache keyed on registry URL |
| Shared browse seam | `src/catalog/catalog_service.rs:1-60` `load_catalog` | The one seam `search`/TUI/MCP share; `CatalogScope::Browse` vs `Complete` must be honored |
| Command pattern | `src/command/login.rs:40-60` args, `:141-185` `read_password` | Template for `grim rate --token-stdin`; `Zeroizing`/`SecretString`, no `--token VALUE` (CWE-214) |
| Forge client | `src/catalog/forge.rs:263-278` `build_client()`, `:292-309` `authorize()` | Redirects hard-disabled (documented CVE-class rationale — must not regress) |
| Token ladder | `forge.rs:37-96` `ForgeContext`/`CiEnv`, `:150-180` `resolve()` | Reuse verbatim; explicit > CI > convention, host-matched |
| Existing POSTs | `forge.rs:507`, `:593`, `:848`, `:928` | Authenticated POST + JSON body already works |
| Report types | `src/api/*_report.rs` | New `rate_report.rs`; single-object shape like `release_report.rs` |
| Exit codes | `login.rs:109-129` | `AuthError=80`, `Unavailable=69`, `OfflineBlocked=81`, `NotFound=79` |

## Component map (indexer, `@grimoire-rs/indexer`)

Source of truth: `.agents/worktrees/grimoire-index` (**the indexer**, despite the name).

| Area | Location | Note |
|---|---|---|
| CLI surface | `src/cli/main.ts:95-232` | `init`, `ci`, `build`, `dev`, `enrich`, `validate` — **no `reconcile`/`tally`** |
| Index compile | `src/data/index.ts:121-167` `compileIndex` | Walks `index/**/metadata.json`, merges `enrich/<ns>/<name>/data.json`, emits exactly one file (`:158`) |
| Closest analog | `src/enrich/index.ts:195-255` `enrichOne` | Digest-gated per-package refresh; template for a tally step. `:266-268` carries a `ponytail:` note that the loop is sequential and needs a worker pool "if an index ever grows into the hundreds" |
| Provider template | `src/validate/adapters/forge.ts:20-28` `Forge` iface, `:146-151` `createForge(kind, config)` | Direct template for `RatingProvider`; GitHub + GitLab impls exist, read-only REST |
| HTTP call site | `src/validate/adapters/http.ts:49-70` `request()`, `:74-83` `requestJson()` | GET-only, `(url, headers)`, no method/body. 8 callers: `forge.ts:68,85,102,117,136`; `registry.ts:81,100,107`. Minimal fix = optional 3rd `{method?, body?}` param, additive |
| CI generation | `src/ci.ts` (`:87` for the `enrich` toggle) | Renders GH Actions / GitLab CI from `index.config.json`; a tally job needs a new config key parallel to `enrich?: boolean` |
| Test harness | `test/validate/helpers.ts:29-51` `stubFetch`; `test/validate/forge.test.ts:15-26` `github()`/`gitlab()` | In-memory fetch fake via `vi.stubGlobal`, records calls. **Caveat:** handler receives `url` only, not `init.body` — GraphQL routes by body on one endpoint, so the handler needs a small extension |
| Runner | Vitest, `test/` mirrors `src/` | No `workspaces`, no `file:` deps declared today |

## Component map (site rendering)

**Fully prerendered. Zero runtime fetch.**

- `src/renderer/astro/lib/data.ts:4-12` — catalog injected at build time via Vite `define` of
  `__GRIMOIRE_DATA__`; module comment: "server-only by construction".
- `src/renderer/types.ts:38-44` `GrimoireData = {config, packages, css}`; `packages` = every
  record from `all.json`.
- `pages/index.astro:176` — `<Catalog client:load packages={packages} …>`; hydrates with the
  catalog embedded in the shipped bundle.
- `pages/p/[...slug].astro:31-36` — `getStaticPaths()` prerenders one HTML file per package.
  `/p/<namespace>/<name>/` is a **frozen public URL contract** (`:2-4`).
- Attach points: `CatalogPackage` (`types.ts:19-31`) for the type; card `meta-row`
  (`Catalog.tsx:577-587`) for display; sort is `type Sort` (`:19`) + a `compare()` branch
  (`:23-28`) + a chip button (`:477-494`); a min-rating filter follows the `kind`-chip shape
  (`:205,422-428`), not the sort shape.
- Convention: **omit the element entirely when a field is absent** — no placeholder text.
  (The "empty panels are omitted / Not provided" wording is `grimoire-vscode`'s webview rule,
  a different repo; the Astro site has its own consistent omit-don't-placeholder idiom.)

**Consequence for the design:** a sidecar `ratings.json` buys the site nothing unless the site
gains its first-ever runtime fetch. Otherwise site ratings refresh only on rebuild, giving grim
and the site different freshness models. Unresolved — the ADR must decide.

## Component map (VS Code extension)

- One spawn call site: `src/grim.ts`, `execFile` only, no shell, `--format json` appended by
  `runJson()` (`:527-554`). Pure argv builders; positional args behind `--` (`:556-570`).
- Envelope parsing `parseReport<T>` (`:386-418`), `GrimResult<T>` union (`:335-358`).
- **No `vscode.authentication` or `SecretStorage` usage anywhere today** — greenfield.
- `MINIMUM_GRIM_VERSION` in `src/installer.ts` is the feature-detection gate; a new
  `grim rate` dependency requires bumping it.
- Not a top-level worktree; only `.agents/worktrees/vscode-deeplink`, a linked worktree off
  `~/dev/grimoire-vscode` on `feat/add-registry-deeplink`.

## Freshness: what is actually in force

`adr_catalog_freshness_revalidation.md` is **Proposed, unimplemented**. Zero
`ETag`/`If-None-Match`/`If-Modified-Since` occurrences anywhere in `src/`.

- In force: `registry_catalog.rs:39` `CATALOG_TTL_SECONDS = 3600`, applied uniformly to every
  source kind via `is_fresh_at` (`:918-922`). Younger than 1h ⇒ serve cache; older ⇒ full
  unconditional rebuild.
- Withdrawn (`Revision 2026-08-12`, `:48-133`): the `Freshness` policy enum (O3), the
  `MAX_STALE_MULTIPLE` ceiling, and the 9th `load_catalog` parameter. The adversarial panel
  found 3 of 18 matrix cells wrong; corrected totals flip to **O1 116 / O3 112** (`:69`), and
  the stated reason to reject O1 (TUI freeze) was false — the freeze is `reload_into` awaited
  inline on the event-loop thread (`src/tui/app.rs:1012-1076`).
- Still standing but unbuilt: **D4** conditional GET — `CatalogFile.validator`,
  `CatalogFile.git_tip`, `CatalogEntry.digest`, all additive-optional. The 3600→300s TTL floor
  drop is **validator-gated**: only sources with a stored validator get the faster window.

**Implication:** there is no revalidation machinery for `ratings.json` to reuse today. Whether
it is cached at all, and under what TTL, is undecided.

## GraphQL: precise cost

| Side | Have | Lack |
|---|---|---|
| grim | Authenticated POST + JSON body at 4 call sites, shared `build_client()`/`authorize()`, TLS roots, redirects disabled | GraphQL query/mutation construction; `{data, errors}` envelope parsing (`get_json`/`send_json` at `forge.rs:1337-1357` assume REST shape + `status.is_success()`) |
| indexer | GET-only `request()`; 8 call sites | POST support (one optional param, additive); nested-envelope response parsing (`requestJson` returns `unknown`, callers use ad-hoc `field()`/`stringOf()` at `forge.ts:38-51`) |

Accurate framing: **a new payload and response-envelope shape over transport that already
exists** on the Rust side; a small additive signature change plus new parsing on the TS side.
Neither side needs a new HTTP client, TLS setup, or auth flow.

## The null-policy split (hard rule, not convention)

- **Cache format** (`CatalogEntry`/`CatalogFile`, on disk): `#[serde(default,
  skip_serializing_if = "Option::is_none")]` — absent fields omitted. See
  `registry_catalog.rs:172-173` (`deprecated`), and the `replaced_by` downgrade comment at
  `:177-178`.
- **`--format json` reports**: always-present-null. `docs/src/json-interface.md:514-520` —
  optional report fields are always present, `null` never an absent key, so consumers can
  distinguish "not applicable" from "older grim". `subsystem-cli-api.md` **bans**
  `skip_serializing_if` in `src/api/`; enforced via hand-written `Serialize` impls
  (`src/api/search_report.rs:89,92,115-116`).

A `rating` field must follow the cache rule on `CatalogEntry` and the always-present-null rule
on every report that surfaces it.

## Prior decisions that bear on this

| ADR | Relevance |
|---|---|
| `adr_projection_over_index.md` | Index is a phone-book of pointers; versions resolved live, never stored. Reinforces `ratings.json` as a sibling artifact rather than folded into `all.json` |
| `adr_git_provenance_annotations.md` | Precedent for opt-in, flag-gated feature with additive `CatalogEntry` fields (`revision`, `created`), no version bump — closest template |
| `adr_catalog_summary_annotation.md` | Template for an optional field flowing end-to-end through the read path |
| `adr_catalog_freshness_revalidation.md` | **Proposed, centrally reversed** — read the `Revision 2026-08-12` block, not the original |
| `adr_multi_registry_mcp.md` | The `catalog_service::load_catalog` seam and per-registry cache keying a ratings cache would join |
| `adr_announce_fork.md` | `ForkPolicy` / self-fork and parent-verification guards, if a write path ever needs non-push-access handling |
| `adr_render_layout_stability.md` | Establishes the additive/migration-promise pattern for any new on-disk artifact |

`.agents/specs/` — none of the six specs touch this domain.

## Dependency graph

```
grim (Rust CLI)
  ├─ publishes INTO → index repos via `grim publish --announce`
  ├─ consumed BY   → indexer's enrich step (spawns `grim describe` / `grim fetch`)
  └─ consumed BY   → grimoire-vscode (execFile, --format json, MINIMUM_GRIM_VERSION-gated)

@grimoire-rs/indexer (npm)
  ├─ builds → index instances (index-site, grimoire-lore)
  └─ coupled to grim only via the `all.json` schema (`schema: 1`) and the enrich call boundary

grimoire-vscode
  └─ depends on grim's CLI/JSON contract only; zero coupling to indexer or index repos
```

No reverse edges.

## Worktree ground truth (verified via `git remote -v` / `rev-parse` / `status`)

| Path | Repo | Branch | State |
|---|---|---|---|
| `.agents/worktrees/grimoire-index` | **`grimoire-rs/indexer`** | `main` | clean |
| `.agents/worktrees/index-site` | `grimoire-rs/index` | `main` | clean |
| `.agents/worktrees/index-ci` | `grimoire-rs/index` (linked worktree off `~/dev/grimoire-index`) | `chore/repo-owned-ci` | clean |
| `.agents/worktrees/index-template` | `grimoire-rs/index-template` | `main` | untracked `.claude/` |
| `~/dev/grimoire-index` | **`grimoire-rs/index`, not the indexer** | `chore/indexer-0.1.9` | dirty |

Three live checkouts of `grimoire-rs/index.git` on three branches. The directory named
`grimoire-index` under `~/dev/` is the *index*; the indexer lives under `.agents/worktrees/`
with the same misleading name.

## Risks / surprises

1. **Site prerendering vs sidecar freshness** — the largest unresolved tension; see above.
2. **No `reconcile`/`tally` verb exists** in the indexer; `enrich` is the closest analog and
   the CI generator needs a new config key to opt a tally step in.
3. **`grimoire-lore` has `trustedBots: []`** — no bot registered, unlike `index-site`. A
   per-instance setup step if lore is meant to get ratings.
4. **Two different additive rules** (cache omit vs report null) are easy to conflate.
5. **`CatalogFile` is `deny_unknown_fields`** — adding a field means older grim rejects a newer
   cache and rebuilds. Documented, deliberate, but a downgrade-behavior choice.
6. **`--offline` semantics**: a rating write while offline should hard-refuse with
   `OfflineBlocked=81`, not degrade silently.
