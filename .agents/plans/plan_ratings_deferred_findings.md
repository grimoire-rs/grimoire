# Plan: Ratings — Deferred Findings

## Status

- **Plan:** plan_ratings_deferred_findings
- **Parent plan:** plan_artifact_ratings (State `done`; three PRs open,
  unmerged — this plan's fixes land on those same branches)
- **Active phase:** 1 — Fixes
- **Step:** finalized
- **Last update:** 2026-08-19 (finalized: 20 commits rebased onto
  origin/main and folded to 6; tree unchanged, `task --force verify` green)
- State:   done
- Tier:    medium
- Updated: 2026-08-19
- Next:    awaiting merge — grimoire#99, indexer#5, grimoire-vscode#18
- Repos:   <!-- frozen; branches already exist and carry open PRs -->
  - `grim`    `/home/mherwig/dev/grimoire` — branch `feat/artifact-ratings`
    @ `88806f8`, PR [grimoire#99](https://github.com/grimoire-rs/grimoire/pull/99)
  - `indexer` `.agents/worktrees/grimoire-index` — branch `feat/artifact-ratings`
    @ `0a6c221`, PR [indexer#5](https://github.com/grimoire-rs/indexer/pull/5)
  - `ext`     `/home/mherwig/dev/grimoire-vscode` — branch `feat/artifact-ratings`
    @ `c49faae`, PR [grimoire-vscode#18](https://github.com/grimoire-rs/grimoire-vscode/pull/18)

## Overview

Nine findings the ratings review panels raised and the build deliberately did
not fix. Every fix lands as new commits on the **existing** feature branch in
its repo, so the one-branch-one-PR-per-repository rule from the parent run
still holds and no new PR is opened.

Three owner decisions are already made and are not re-litigated below:

- **D-1** — the acceptance-level `--up` happy path gets its fake forge from
  **loopback plain HTTP**, mirroring the always-on loopback in
  `GRIM_INSECURE_REGISTRIES`.
- **D-2** — the **site adopts grim's** deprecated-artifact rule (hide by
  default, offer a toggle). grim's shipped default is frozen by Principle 9
  and does not move.
- **D-3** — scope is **all nine** findings, across all three repos.

## Objective

Close every deferred finding from the ratings review with a regression test
that fails without the fix, without breaking a single shipped contract.

## Scope

### In Scope

- The five findings on [grimoire#99](https://github.com/grimoire-rs/grimoire/pull/99).
- The two on [indexer#5](https://github.com/grimoire-rs/indexer/pull/5).
- The two on [grimoire-vscode#18](https://github.com/grimoire-rs/grimoire-vscode/pull/18).
- A `vendor-capability-watchlist.md` entry for every upstream version claim
  the docs make, date-stamped.

### Out of Scope

- Enabling ratings on `index.grimoire.rs` or provisioning any credential —
  owner actions, untouched here as in the parent run.
- Any change to `all.json`, to a shipped CLI flag or exit code, or to a
  `src/api/` field's type or nullability.
- The site's own sort semantics beyond the deprecated-artifact rule.

## Findings and their fixes

### F-1 — a 503 on the sidecar caches an empty rating map (Block-adjacent, grim)

**Where.** `src/catalog/index_source.rs:253` `fetch_ratings`, and its caller
`fetch_index_entries:217`.

**What happens now.** `fetch_ratings` collapses *every* failure to
`BTreeMap::new()`: a 404, a transport error, a 5xx, an unparseable body and a
future `schema_version` are one outcome. `build_from_index`
(`src/catalog/registry_catalog.rs:687`) writes the resulting catalog to the
cache under the normal 1-hour TTL. So one 503 on `stats.json` — with
`all.json` fetching fine — publishes "nothing is rated" into the cache and
every surface reads unrated for the full TTL.

This is exactly what R-2 forbids the publisher from doing, on the read side.
The indexer's `loadSeed` already branches on status for this reason; the
consumer does not.

**Fix.** Give the read side the same branching, then carry forward.

1. `parse_ratings` and `fetch_ratings` return `Option<BTreeMap<..>>`:
   - `Some(map)` — a **completed observation**: a 404 (`Some(empty)`, the
     normal case for an index without ratings) or a 2xx that parsed.
   - `None` — **nothing was observed**: transport error, 5xx, a 2xx that did
     not parse, or a `schema_version` from the future.
2. `fetch_index_entries` takes `previous: Option<&BTreeMap<String, CatalogEntry>>`
   (the prior cache's entries, which the caller already holds — see
   `registry_catalog.rs:603` and `:633`). On `None`, each entry's rating comes
   from `previous.get(&key).and_then(|e| e.rating.clone())` instead of from
   nothing. Keys match: both sides key by `CatalogEntry::repo()`
   (`registry_catalog.rs:241`).
3. A cold cache plus an unobserved sidecar is still unrated — that is honest,
   nothing is known — but it must not *overwrite* what was known.

`Option` rather than a `Result` with a new error variant: a failed sidecar is
not a catalog-build failure, and `fetch_index_entries` must keep succeeding on
the strength of `all.json` alone.

**Tests.**

| Test | Where | Proves |
|---|---|---|
| `a_404_sidecar_is_a_completed_observation_of_nothing` | `index_source.rs` | 404 ⇒ `Some(empty)` |
| `a_transport_failure_is_unknown_not_empty` | `index_source.rs` | 5xx / transport ⇒ `None` |
| `an_unparseable_sidecar_is_unknown_not_empty` | `index_source.rs` | rewrites the existing assertion, which currently expects an empty map |
| `a_future_schema_version_is_unknown_not_empty` | `index_source.rs` | same |
| `an_unobserved_sidecar_carries_the_previous_ratings_forward` | `registry_catalog.rs` | the actual R-2 property |
| `a_404_sidecar_clears_a_previously_rated_entry` | `registry_catalog.rs` | carry-forward does **not** become "never forget" — a completed empty observation still empties |
| `a_sidecar_503_leaves_a_warm_cache_rated` | `test/tests/test_index_source.py` | end to end, over the existing loopback server |

Two existing unit tests assert an empty map for the unparseable and
future-schema cases. Their **meaning** changes, not their subject; both are
rewritten in place with the reason in the test body.

### F-2 — `--up`'s happy path is never executed end to end (Warn, grim)

**Where.** `test/tests/test_rate.py` — `--up` appears once, in
`test_up_and_remove_together_is_a_usage_error`, expecting 64.

**Why it is not already covered.** `rating_provider::graphql_endpoint`
(`src/catalog/rating_provider.rs:256`) hardcodes `https://`, and the rating
endpoint is deliberately never index-supplied — an index carries no host, so
index content has no path to a credential destination. `publish --announce`'s
manifest-injected `api_url` is therefore not a pattern this path may copy.

**Fix (D-1).** `graphql_endpoint` emits `http://` for **loopback hosts only** —
`127.0.0.1`, `localhost`, `::1`, bare or with a port — and `https://` for
everything else. This mirrors the always-on loopback set in
`GRIM_INSECURE_REGISTRIES` (`AGENTS.md`), an accepted decision in this
codebase, and narrows nothing else: `--token-host` still gates, and a
non-loopback host is unreachable over plain HTTP as before.

Then the acceptance suite drives the full CLI against a fake GraphQL server on
loopback, in the shape `test_publish_announce.py` already uses for forge APIs:

```
GRIM_RATING_HOST=127.0.0.1:<port> grim rate <ref> --up --yes \
    --token-stdin --token-host 127.0.0.1:<port>
```

**Tests.**

| Test | Where | Proves |
|---|---|---|
| `graphql_endpoint_is_plain_http_on_loopback_only` | `rating_provider.rs` | the scheme rule, including that a lookalike (`127.0.0.1.evil.example`) stays https |
| `test_an_up_vote_succeeds_and_reports_the_forge_state` | `test_rate.py` | argv → confirm → resolve → mutate → report, exit 0, `viewer_up: true` |
| `test_a_remove_retracts_and_reports_not_voted` | `test_rate.py` | the toggle's other arm |
| `test_a_vote_records_the_account_id_in_votes_json` | `test_rate.py` | the tri-state store is written from the response |

The docstring at the top of `test_rate.py` — "The one thing no test here may
do is reach a real forge" — is amended to say the fake forge is loopback and
why that is not a real forge.

### F-3 — `load_or_cold`'s `# Errors` contradicts its body (Suggest, grim)

**Where.** `src/catalog/registry_catalog.rs:409-413`.

The doc says "[`CatalogError`] for a read failure, or any load failure while
offline". The body's `Err(e) if !offline` arm converts *every* online failure,
read failures included, to `Ok(None)`. Only the offline clause is true.

**Fix.** Doc-only:

```
/// # Errors
///
/// [`CatalogError`] for any load failure while offline. Online there is
/// none: an unreadable cache is a cold cache.
```

No test — a doc comment is not a contract a test can hold. Caught by review.

### F-4 — deprecated artifacts sort differently in grim and on the site (Warn, indexer)

**Decision (D-2).** The **site** adopts grim's rule.

**Where.** `src/renderer/astro/components/Catalog.tsx` and the sort comparator
it feeds.

**Fix.** Deprecated artifacts are filtered from the browse by default rather
than sunk to the end, and a control restores them — the site's equivalent of
grim's `--show-deprecated` and the TUI's `h`. grim is untouched: its default
result set is a shipped contract and Principle 9 freezes it.

**Tests.** `test/renderer/sort.test.ts` gains a case per sort mode asserting a
deprecated entry is absent by default and present with the toggle on, and the
existing "sunk to the end" assertions are rewritten rather than deleted.

### F-5 — two upstream version claims were never re-verified (Warn, grim + indexer)

**Where.** `docs/src/ratings.md:247` (GHES has shipped Discussions since 3.6)
and `:300` (GitLab token expiry mandatory since 16.0 — 365 days, 400 on 17.6
and later). Both came from the planning research and were carried through
unchecked; the same claims are echoed in the indexer README.

**Fix.**

1. Verify each against upstream documentation and record the URL and the date
   checked.
2. Correct the docs if either is wrong.
3. Add a dated row per claim to `.claude/rules/vendor-capability-watchlist.md`
   — the rule that exists precisely so a vendor claim is re-verified before it
   is patched around, rather than aging silently in prose.

**Tests.** None — a version claim about someone else's product is not
testable here, which is why it belongs on the watchlist.

### F-6 — the seed URL is derived twice, from two different sources (Warn, indexer)

**Where.** `src/cli/ratings.ts:113-139` reads the seed from
`<index.config.json site>/stats.json` and refuses without an explicit `site`.
The CI guard reads a different source: `steps.pages.outputs.base_url`
(`templates/ci/github-ratings-seed.yml:24`) and `CI_PAGES_URL`
(`templates/ci/gitlab-ratings-seed.sh:33`).

**Why it matters.** They can disagree. When they do, the guard checks URL A
and passes while the reconcile fetches URL B and gets a 404 — which is a legal
empty seed, so the run *completes* and publishes a sidecar built from only
what this pass observed. The guard exists to stop exactly that and cannot see
it, because it was looking at a different URL.

**Fix.** One source. The generated seed step takes the configured `site`
baked in at render time, and the platform variable becomes a **cross-check**:
when it is present and disagrees with `site`, the job fails naming both,
rather than silently preferring one.

**Tests.** `test/ratings/ci.test.ts` — the rendered job carries the configured
`site`; a rendered job whose platform variable disagrees fails; and a run
where the two agree is unchanged.

### F-7 — one unidentified flaky test (Warn, ext)

**What was observed.** Four consecutive `xvfb-run -a npm test` runs on the
final tree: three clean at 1008 passing, one at 1007 + 1 failing. The name was
lost to a truncated capture and did not reproduce.

**Fix.** Identify it before guessing at it: run the suite under a
machine-readable reporter to a file, repeatedly (20 runs, or until one fails),
and read the failure out of the file rather than a pipe. The suite carries
several debounce-timed assertions (700 ms – 4.3 s waits) and those are the
first suspects, but that is a hypothesis to test, not a diagnosis.

Once named: fix the race, or — if it is a genuine timing dependency that
cannot be made deterministic — pin the wait to a fake clock. **Not** a retry
and not a skip; a flaky test that is retried is a test that has stopped
gating.

**Tests.** The fixed test is its own evidence: 20 consecutive green runs.

### F-8 — `views/vote.ts` at 39% line coverage (Warn, ext)

**Where.** `src/views/vote.ts`, uncovered lines 126–140 and 158–195 — the
command's own error and cancellation branches. The credential ladder and both
host anchors are already covered by `src/test/rating.test.ts`.

**Fix.** Cover the uncovered branches: a cancelled quick-pick, a non-zero exit
from grim, a grim that is missing, and a malformed report. Each asserts what
the user sees and that no vote was cast.

**Tests.** Cases added to `src/test/rating.test.ts`; the target is that the
error paths execute, not a coverage percentage.

## Technical Approach

### Key decisions

| # | Decision | Why |
|---|---|---|
| D-1 | Loopback plain HTTP for the rating endpoint | Mirrors the accepted always-on loopback in `GRIM_INSECURE_REGISTRIES`; the alternative was a test-only seam in production code or self-signed TLS in the pytest suite |
| D-2 | The site adopts grim's deprecated rule | grim's default result set is frozen by Principle 9; moving the site is the only non-breaking convergence |
| D-3 | `Option<BTreeMap>` for the sidecar, not a new error variant | A failed sidecar is not a catalog-build failure — `all.json` alone decides that |
| D-4 | Carry forward from the prior cache, not "never empty" | A *completed* observation of nothing must still empty, or a retracted rating would live forever |
| D-5 | Fixes land on the existing branches | Keeps one PR per repository, the constraint the parent run was given |

## Work packages

| WP | Scope | Expected files | Size | Wave | Depends on | Review | Status |
|---|---|---|---|---|---|---|---|
| DF-A | F-1 sidecar status branching + carry-forward | **grim** `src/catalog/index_source.rs`, `src/catalog/registry_catalog.rs`, `test/tests/test_index_source.py` | M | 1 | — | panel | merged |
| DF-B | F-2 loopback endpoint + `--up` end to end | **grim** `src/catalog/rating_provider.rs`, `test/tests/test_rate.py` | M | 1 | — | panel | merged |
| DF-C | F-3 doc correction | **grim** `src/catalog/registry_catalog.rs` | S | 2 | DF-A | self | merged |
| DF-D | F-5 verify upstream claims + watchlist | **grim** `docs/src/ratings.md`, `.claude/rules/vendor-capability-watchlist.md`, `.claude/rules.md` | S | 1 | — | light | merged |
| DF-E | F-6 one seed URL, one `site` policy | **indexer** `src/config.ts`, `src/cli/{ratings,init}.ts`, `templates/ci/*-ratings-seed.*`, `templates/ci/github-pages.yml`, `test/ratings/ci.test.ts` | M | 1 | — | panel | merged |
| DF-F | F-4 site/grim deprecated parity | **indexer** `src/renderer/astro/components/Catalog.tsx`, `test/renderer/sort.test.ts` | M | 1 | — | light | merged |
| DF-G | F-7 identify and fix the flake | **ext** — no change; unreproduced in 31 runs | M | 1 | — | light | merged |
| DF-H | F-8 cover vote.ts error and wiring branches | **ext** `src/test/rating.test.ts` | S | 1 | — | self | merged |

DF-C is wave 2 only because it edits a file DF-A owns. Everything else is
file-disjoint and runs in one wave.

**Critical path:** DF-A → DF-C.
**Shippable after wave:** 1 (DF-C is a doc comment).

## Testing Strategy

Every fix carries a test that fails without it. F-3 and F-5 are the two
exceptions, and both are exceptions for a stated reason: a doc comment is not
a contract a test can hold, and a version claim about another vendor's product
is not testable in this repo — which is why it goes on the watchlist instead.

Two mutation checks per repo against real source before the branch is
re-pushed, pasted into the execution report.

## Verification

| Repo | Gate |
|---|---|
| grim | `task --force verify` and `task catalog:verify` |
| indexer | `npm run lint && npm run typecheck && npm test` |
| ext | `npm run check`, then `xvfb-run -a npm test` ×20 for DF-G |

## Backwards compatibility (Principle 9)

| Change | Why it is additive |
|---|---|
| `fetch_ratings` → `Option` | Private to the module; no reported field changes type or nullability |
| Carry-forward from the prior cache | The cache struct is unchanged; only which value is written to an existing field |
| Loopback plain HTTP | Widens nothing for any non-loopback host; no flag, exit code or JSON field moves |
| Site hides deprecated | The site is a rendered surface, not a frozen contract; `all.json` is untouched |
| Seed URL single source | Generated CI only; an index that regenerates gets the fix, one that does not is unaffected |

`all.json` stays byte-identical — the indexer's golden test is the assertion
that says so, and it is not touched by any WP here.

## Rollback

Every WP is one commit on a branch that has not been merged. Rollback is
`git revert` of that commit, or dropping it before the branch is re-pushed.
No migration, no state change, nothing published.

## Risks

| Risk | Mitigation |
|---|---|
| DF-B widens where a credential may be sent | Loopback only, exact-match host set, `--token-host` still gates. Security review on the WP. |
| DF-A's carry-forward hides a genuine retraction | D-4: a completed observation of nothing still empties. Test `a_404_sidecar_clears_a_previously_rated_entry` is that assertion. |
| DF-G finds nothing in 20 runs | Report it as unreproduced with the run count rather than closing it as fixed. A flake that cannot be named is not a flake that has been fixed. |
| Rewriting two existing tests in DF-A looks like fitting the test to the code | Each rewrite cites the contract clause saying the old assertion was wrong, in the test body. |

## Checklist

### Before starting

- [ ] All three branches at the SHAs in the Status block, working trees clean
- [ ] No PR merged in the meantime (a merged PR changes the base)

### Before re-push

- [ ] Every gate green, output pasted
- [ ] Each PR body's deferred-findings section updated to say what was fixed
      and what, if anything, still stands
- [ ] `--force-with-lease`, one branch per repo, no new PR
