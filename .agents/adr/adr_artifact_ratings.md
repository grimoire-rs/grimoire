# ADR: Forge-Backed Artifact Ratings

<!--
Architecture Decision Record
Owner: /hex-architect run 2026-08-17/18, Design phase (architect)
Handoff to: /hex-plan, /builder, /security-auditor
Companion: .agents/specs/design_artifact_ratings.md (C4, data flow, algorithms)
-->

## Metadata

**Status:** Accepted
**Date:** 2026-08-18
**Deciders:** Owner (accepted 2026-08-18), architect (this record)
**Beads Issue:** N/A
**Related Issue:** [grimoire-rs/grimoire#82](https://github.com/grimoire-rs/grimoire/issues/82) — Ratings for catalog artifacts
**Tech Strategy Alignment:**
- [x] Decision follows Golden Path in `.claude/rules/product-tech-strategy.md` (Rust 2024 + Tokio in grim, existing TypeScript/Vitest in the indexer, GitHub Actions + GitLab CI, no new runtime)
**Domain Tags:** data | integration | security | api
**Supersedes:** —
**Superseded By:** —

## Context

The catalog has no quality signal. A browser of `index.grimoire.rs`, the TUI,
or `grim search` sees a name, a summary, and a kind — nothing that separates
an artifact three teams depend on from one published and abandoned the same
afternoon. Issue [#82](https://github.com/grimoire-rs/grimoire/issues/82) asks
for ratings.

The constraint that shapes every part of the answer is the product thesis:
*"Storage is any OCI registry — GHCR, Docker Hub, or your own. There is no
Grimoire service to sign up for"* ([`product-context.md`](../../.claude/rules/product-context.md)).
An index is a repository someone stands up with `npx @grimoire-rs/indexer init`
and serves from GitHub or GitLab Pages. Whatever rating mechanism ships must
work identically for a private, air-gapped, self-hosted index — otherwise it
is a feature of the public index rather than a feature of Grimoire.

The second constraint is Principle 9. The project is stabilizing toward 1.0
with breaking changes prohibited outright — not "until the next major", but
permanently ([`docs/src/stability.md`](../../docs/src/stability.md)). Every
artifact this decision touches is either already frozen (`all.json` transport,
`--format json` reports, the CLI surface) or becomes frozen the moment it
ships (`stats.json`).

Five research artifacts precede this record and are not restated here:
[`research_rating_backends.md`](../research/research_rating_backends.md)
(ecosystem survey, verified forge API facts, performance numbers),
[`research_rating_architecture_map.md`](../research/research_rating_architecture_map.md)
(code map, every seam, worktree ground truth),
[`research_rating_schema_compat.md`](../research/research_rating_schema_compat.md),
[`research_rating_security.md`](../research/research_rating_security.md), and
[`research_rating_operability.md`](../research/research_rating_operability.md).

## Decision Drivers

- **Zero infrastructure cost.** No service, no database, no OAuth application,
  no new uptime dependency — for the public index *and* for every self-hosted one.
- **Principle 9.** `all.json` byte-identical; every new field additive and
  optional; `stats.json` designed on day one for a consumer that will never
  be allowed to break.
- **Absent is first-class.** No file, no entry, no field must never be an
  error — this single invariant carries offline use, older indexes,
  non-participating indexes, and every failure mode of the write path.
- **Browsing must not depend on a write path.** Reads are static and cached;
  writes are live, optional, and allowed to fail without degrading reads.
- **Reversibility.** Package registries deliberately avoid ratings
  ([crates.io default-ranking RFC](https://rust-lang.github.io/rfcs/1824-crates.io-default-ranking.html));
  extension marketplaces have them. The evidence does not settle which shape
  Grimoire is, so the mechanism must be cheap to remove or replace.

## Industry Context & Research

**Research artifacts:** the five listed above.

**Trending approach:** *forge as database* — the pattern
[giscus](https://github.com/giscus/giscus) proved across thousands of sites:
the forge supplies storage, identity, moderation, and abuse reporting; the
project operates nothing. The same pattern underlies every reconciler bot
surveyed (Renovate, Dependabot, release-please, `actions/stale`,
all-contributors): none keeps an external record of what it owns — the forge
object *is* the state.

**Key insight:** there is no generic "rating microservice". The category does
not exist as a deployable component. Every candidate
([Remark42](https://remark42.com/docs/manuals/kubernetes/),
[Fider](https://github.com/getfider/fider),
[Open VSX](https://www.eclipse.org/legal/open-vsx-registry-faq/)) is a
comment, feedback, or registry product bent into the role, and each adds a
service, a database, and an OAuth client to a stack that currently has none.
Meanwhile both target forges already expose a native upvote primitive and a
toggle mutation — GitHub Discussions `upvoteCount` / `addUpvote` /
`removeUpvote`, GitLab work items' AwardEmoji widget `upvotes` /
`awardEmojiToggle` — so the two providers converge on nearly identical
client shapes.

**Second insight, from the compatibility axis:** the one mechanical rule
worth carrying from every ecosystem that got this wrong is *never change what
a field can contain; add a new field instead*. crates.io's `features2` split
exists because an early version widened `features`'s meaning and pre-1.19
Cargo could not parse the result — even a client willing to *ignore* the new
syntax could not, because the old field's values became unparseable
([Cargo Book](https://doc.rust-lang.org/cargo/reference/registry-index.html)).

## Considered Options

### Option 1: Forge-backed ratings, static read path, live write path

**Description:** A bot creates one thread per artifact on whichever forge
hosts the index (GitHub Discussion or GitLab work item), embedding a
`grim-ref` marker in its own post body. A CI job reconciles missing threads,
tallies the native upvote counters, and publishes a static `stats.json`
beside `all.json`. Reads join it client-side by ref. Writes go live from
`grim rate` to the forge's toggle mutation using the user's own forge
credential.

| Pros | Cons |
|------|------|
| No service, no DB, no OAuth app — works identically for a self-hosted index | Rating freshness is bounded by the tally cadence, not real-time |
| Identity, moderation, abuse reporting, and rate limiting are inherited free | GitHub has no native self-upvote block (GitLab does) |
| Read path is a static file: offline-safe, CDN-cacheable, no uptime we own | Thread creation is capped at 500/hour — a real cold-start cost past ~1k artifacts |
| The seam is data, so a future service replaces the forge with no client change | Marker trust is a genuine attack surface that must be designed, not assumed |
| Both providers ship day one from one nearly-identical shape | Requires Discussions enabled + an announcement category as a manual prerequisite |

### Option 2: Self-hosted rating service

**Description:** Operate a small service (own schema, own database, own OAuth
client per forge) that stores votes and serves aggregates.

| Pros | Cons |
|------|------|
| One-vote-per-human enforceable properly, with real identity, not a proxy for it | Every self-hosted index would have to operate one too, or ratings become a public-index-only feature — this breaks the product thesis, not just a preference |
| Arbitrary schema: weighted scores, per-version ratings, review text, retraction audit | Requires an OAuth application registration per forge — an admin step on GHES and self-managed GitLab, which is exactly the friction `grim-indexer init` exists to remove |
| No 500/hour creation ceiling; no marker-trust attack surface at all | New uptime dependency: browsing degrades when it is down |
| Live counts without a rebuild — the freshness story is strictly better | Directly contradicts "There is no Grimoire service to sign up for" |

This is the correct long-run architecture *at scale*, and the decision below
prices in migrating to it. It is wrong *now* because the cost it imposes is
not "we run a server" but "everyone who runs an index runs a server".

### Option 3: Adopt Open VSX

**Description:** Adopt the Eclipse Open VSX registry, which ships Ratings &
Reviews built in, EPL-2.0, self-hostable.

| Pros | Cons |
|------|------|
| The closest working precedent — ratings *and* reviews, already built and proven at marketplace scale | Adopting it means adopting their registry model, not adding ratings to ours |
| Real moderation tooling and a mature data model | Replaces the OCI-registry-as-storage thesis that is the entire product |
| Self-hostable, so the parity story is at least arguable | Every published artifact, annotation, and lockfile pin would need re-homing — a Principle 9 catastrophe, not a migration |

### Option 4: No ratings — popularity signal only

**Description:** Ship no rating mechanism. Rank and display a passive
popularity signal (download counts, source-repo stars) instead, following
crates.io, npm, and PyPI, all of which deliberately avoid quality scores.

| Pros | Cons |
|------|------|
| Zero attack surface, zero credential handling, zero new artifact, zero new CI | **The signal is not actually available.** OCI registries expose no public download API — GHCR has none — so "downloads" cannot be computed at all |
| Strong prior art: crates.io's own research found people judge crates by *documentation quality*, not scores | The remaining fallback is source-repo stars, **rejected on merit, not merely deferred** — see below |
| Genuinely the cheapest correct answer if the signal existed | Delivers nothing against issue #82 |

Kept in the matrix as the honest cost baseline. It is rejected on fitness,
not on cost — and the fact that it *nearly wins the weighted matrix anyway*
is the single most important finding in this record.

### Option 5: Reuse a comment engine (Remark42 / giscus)

**Description:** Deploy Remark42 (Go + DB + custom OAuth2 providers, Helm
chart available) or embed giscus, and read vote counts out of it.

| Pros | Cons |
|------|------|
| Voting works today; giscus is battle-tested on thousands of sites | giscus gives a comment *UI*, not counts — we would be scraping its storage, which is GitHub Discussions anyway, i.e. Option 1 with an extra dependency |
| giscus already solved the marker-collision problem (hash-based body matching, shipped 2022-07-23) | Remark42 carries all of Option 2's costs — service, DB, OAuth client — with less control over the schema |
| Mature moderation and spam handling | A comment system bent into a rating field; its GHE self-hosting path is where people get stuck |

**Source-repo stars are rejected permanently, not deferred.** The design gate
put star-seeding out of v1 scope; the reason it should never return is
structural, and is recorded here so it is not re-proposed as a cheap
cold-start fix.

Grimoire's own thesis is that a skill teaching a tool belongs *in that tool's
repository* — the Terraform skill ships with Terraform, not in a curated
skills repo. Two consequences follow, and the second is fatal:

1. **Stars measure the product, not the skill.** A 40k-star product with a
   mediocre one-paragraph skill outranks a 200-star project shipping an
   excellent one. The number is a proxy for something the user is not choosing
   between.
2. **Stars have zero variance across skills from one repository.** A monorepo
   publishing five skills gives all five an identical count. A signal that is
   constant across the items it is meant to rank does not rank them — it
   ranks their *containers*, which the user already knows about.

A vote attaches to the artifact. A star attaches to whatever repo happens to
hold it. That difference is the whole reason this ADR exists, and it is why
"just use stars" is not a cheaper version of this decision.

**Download counts are a separate axis, deliberately not in scope here** —
tracked as [#89](https://github.com/grimoire-rs/grimoire/issues/89) rather
than folded in. They are not the fallback
Option 4 assumed: the capability is per-backend and the default backend is the
one that lacks it.

| Backend | Per-artifact download signal |
|---|---|
| **GHCR** (grim's default) | **None usable.** No supported REST or GraphQL field; the community answer is scraping the web UI ([community#146215](https://github.com/orgs/community/discussions/146215), third-party workarounds like [ghcr-pulls](https://github.com/ipitio/ghcr-pulls)) |
| **GitLab registry** | **None.** Still an open feature request ([gitlab#15807](https://gitlab.com/gitlab-org/gitlab/-/issues/15807)) |
| **Docker Hub** | `pull_count`, but repository-level, not per tag or per artifact |
| **JFrog Artifactory** | **Yes, mature** — `stat.downloads` via AQL, genuinely per-artifact |
| **Harbor / Quay** | Per-repository pull counts (vendor extension) |

No OCI distribution-spec endpoint exposes this; every one of the above is a
vendor extension, which means any future support is an absent-is-first-class
per-backend capability of exactly the shape `stats.json` already uses — and
therefore composes with this decision rather than competing with it. It is
also a noisier signal than it looks: CI pulls dominate, so a download count
measures automation, not human judgement.

## Decision Outcome

**Chosen Option:** Option 1 — forge-backed ratings, static read path, live
write path, both providers from day one, with the provider seam expressed as
**data** (`stats.json`) rather than as an abstraction layer.

### Weighted trade-off matrix

Criteria weighted by how much this project actually cares, not by how much a
generic project would. Scores 1–5, higher is better. Maximum 85.

| Criterion | Weight | Why this weight | O1 forge | O2 service | O3 Open VSX | O4 none | O5 comment engine |
|---|---|---|---|---|---|---|---|
| Fitness — delivers a per-artifact signal users can act on | ×3 | The ask | 4 | 5 | 5 | 1 | 4 |
| Infrastructure cost | ×3 | The product thesis | 5 | 1 | 1 | 5 | 1 |
| Principle 9 / compat risk | ×3 | Breaking changes are prohibited | 5 | 4 | 1 | 5 | 4 |
| Self-hosted-index parity | ×2 | A private index must work identically | 4 | 1 | 2 | 5 | 1 |
| Security / abuse surface | ×2 | A rating is a trust signal, therefore a target | 3 | 4 | 4 | 5 | 3 |
| Reversibility | ×2 | The registries-vs-marketplaces evidence is unsettled | 5 | 3 | 1 | 5 | 3 |
| Freshness / UX | ×1 | Votes are not latency-sensitive | 3 | 5 | 5 | 1 | 5 |
| Implementation cost | ×1 | Real, but the smallest term | 3 | 1 | 1 | 5 | 2 |
| **Total** | | | **72** | **52** | **41** | **69** | **48** |

**Rationale.** Option 1 wins, but the margin over Option 4 — doing nothing —
is three points out of eighty-five. That thinness is a finding, not noise:
it says the case for ratings rests almost entirely on the *fitness* column,
and that every point of added cost, complexity, or coupling erodes it
directly. Three consequences follow and are binding on the design:

1. **The mechanism stays cheap.** No signed aggregate (the index itself
   carries no signature scheme — `quality-security.md`: *"No signature
   verification exists"* — so signing the smaller, less valuable artifact
   while the larger one stays unsigned is theatre). No custom anti-abuse
   system. No second OAuth application. No new repository to operate.
2. **The mechanism stays reversible** — with one honest caveat. The seam is
   a data contract, so retreat is stopping the tally and unpopulating the
   field; replacing the backend is one `match` arm plus one provider
   implementation. The caveat: a shipped `--format json` field can never be
   *removed* (`stability.md#frozen-additive-fields`: a minor "never removes
   one"), so retreat leaves `rating` serialising as a permanent `null` — which
   is already the documented "not applicable" state every consumer must
   handle. Threads created on the forge likewise cannot be un-created. Both
   are cheap; neither is nothing, and scoring reversibility 5/5 assumes
   exactly this reading.
3. **The revisit triggers below are load-bearing**, not decoration. When one
   fires, re-run this matrix — the answer may legitimately change.

Option 2 is the right architecture at scale and the wrong one now, for a
reason sharper than cost: it would make ratings a property of the public
index rather than of Grimoire, because every self-hosted index would have to
stand up the same service and register the same OAuth app. Option 3 replaces
the product. Option 4's cheapness is real but its premise is false — the
passive signal it would substitute does not exist on OCI registries. Option 5
is Option 1 wearing a dependency.

### Design decisions

**D1 — Site freshness: rebuild-to-refresh, no runtime fetch.**
The index site is fully prerendered; the catalog is baked into the bundle at
build time via a Vite `define` of `__GRIMOIRE_DATA__`, and
`src/renderer/astro/lib/data.ts` is documented *"server-only by
construction"*. Rather than give the site its first-ever runtime fetch, the
tally job runs **as a pipeline stage before the site build**, writes
`stats.json`, and `buildSite` joins it onto `packages` by ref at build
time. The site therefore gains an optional `CatalogPackage.rating` in props
it already receives, and no new fetch, loading state, or layout shift.

This does not leave grim and the site on different freshness models — it
puts them on the same one. Both read the same published `stats.json`; the
site's copy is exactly as fresh as the last tally, and grim's is that plus
its own ≤1h catalog cache window. The cost is stated plainly: a vote cast at
00:05 is invisible everywhere until the next tally-and-deploy (hourly).
Reversibility is preserved — `stats.json` is a published URL either way, so
the runtime-fetch option remains available later as a purely additive change
with no data-shape movement.

**D2 — `stats.json` gets no cache of its own.**
It is fetched in the same pass as `all.json`, by the same code path
(`src/catalog/index_source.rs` `fetch_index_entries`), joined by ref, and
lands as `CatalogEntry.rating` inside the **existing** `CatalogFile` envelope
under the existing `CATALOG_TTL_SECONDS = 3600`. No second cache file, no
second TTL, no new envelope, no new freshness model. `GRIM_OFFLINE` inherits
the catalog's behaviour verbatim — zero new code. A 404, a transport error,
or a parse failure on the sidecar degrades to *no ratings* at `debug` level;
only the `all.json` fetch decides whether the catalog build succeeds.

Nothing here re-decides
[`adr_catalog_freshness_revalidation.md`](./adr_catalog_freshness_revalidation.md),
whose central decision was reversed on 2026-08-12 (`O1 + O5`: TTL stays a
gate, revalidation becomes cheap). Because `stats.json` is fetched at the
same seam, it inherits that ADR's still-standing D4 conditional-GET
machinery for free whenever it is built — no separate design needed.

**Scope boundary:** ratings apply to **HTTP index sources only**
(`SourceKind::IndexHttp`). A git-transport index has no committed
`stats.json` (D8 forbids committing it), and an OCI `_catalog` source has
no index document to hang a sidecar off. Both read as unrated — which is
absent-is-first-class, not a gap.

**D3 — `provider` is a plain string in the top-level block, not a tagged
enum.** The compatibility research flagged `#[serde(other)]`'s hard
constraint: the fallback must be a **unit** variant, so a future third
provider's fields are silently dropped. This design removes the hazard
instead of mitigating it: `target` and `url` are hoisted **out** of the
provider object and onto the entry, so the discriminator carries *no
read-path data at all*. What remains is a single string naming which write
mutation to issue — and both mutations need only the opaque `target`
(`addUpvote(input: {subjectId})`, `awardEmojiToggle(input: {awardableId,
name})`), so there are no provider-specific fields left to lose.

Consequently `provider` is one string in the top-level block (one index, one
forge), not a per-entry object; a per-entry override remains available as a
purely additive field if an index ever spans two forges. Rust carries it as
`String` and converts at the single write-path dispatch, where an
unrecognised value produces `UnsupportedProvider("forgejo")` with the raw
value preserved in the message — an unknown provider degrades to *readable
but not writable*, which is exactly correct. TypeScript carries `provider?:
string`; the site never dispatches on it.

**This is a deliberate departure from `research_rating_schema_compat.md`'s
recommendation (2), and from the "enum + match" phrasing in the settled
brief.** The research recommended a tagged enum because it assumed
provider-specific fields; the opaque-handle contract removes them, and once
they are gone a tagged enum is machinery guarding against a hazard that no
longer exists. Reviewers should read this as a resolution of Hazard 1, not
an oversight of it.

**D4 — Marker authority is a named invariant, not an implementation
detail.** Announcement-category locking gates only *top-level* posts; anyone
with read access can post a reply, and nothing stops a reply containing a
forged `grim-ref` marker pointing at a different package. Left unaddressed,
one public comment redirects a popular artifact's organic upvotes onto a
typosquat. This is Block-tier and is specified as:

> **Invariant R-1 (Marker Authority).** A `<!-- grim-ref: <ref> -->` marker
> establishes a `ref → target` binding **if and only if** all three hold:
> 1. it appears in the **body of a top-level thread object** — a GitHub
>    Discussion's own body, a GitLab work item's own description — never in a
>    comment, reply, or note;
> 2. that object's **author account id** equals a configured bot id — the
>    immutable numeric id, never the login. This is the index tooling's own
>    existing doctrine, stated verbatim in
>    `src/validate/core/ownership.ts`: *"ids are immutable and rename-proof,
>    so every comparison that decides anything compares ids"*;
> 3. the marker is the **first** match of an anchored pattern in that body;
> 4. the thread **still lives in the configured container** — its repository
>    (GitHub `repository.id`) or project (GitLab `project.id`) equals the
>    configured index repo, *and* its category/work-item type equals the
>    configured `container`. Compared by **immutable id**, never by name.
>
> A `ref` resolving to **more than one** authorized thread is a **conflict**:
> it contributes **zero** votes and logs both thread URLs. Silently picking
> one is how giscus's duplicate-creation bug
> ([giscus#738](https://github.com/giscus/giscus/issues/738)) becomes
> invisible.

The announcement-category lock is retained as defence in depth. Clause 2 is
the mechanism; **clause 4 closes thread mobility**, which clauses 1–3 do not.
A GitHub Discussion can be *transferred* to another repository or *converted
to an issue*, and a GitLab work item can be *moved* between projects — each
carries its body and its original author id with it. Without clause 4 a
bot-authored thread that has left the index repo still satisfies every other
condition, so an attacker who can get a thread transferred into a repo they
control inherits a genuine `ref → target` binding and every upvote cast
against it. Clause 4 makes the container part of the identity, so a moved
thread simply stops being authoritative and its ref reads as unrated.

R-1 is testable without a network: the in-memory provider fake returns
(a) a marker in a stranger's reply, (b) a marker in a stranger's top-level
post, (c) a marker in a bot top-level post, (d) two bot posts carrying the
same marker. Assert 0, 0, counted, 0-plus-warning.

**D5 — Bot identity per forge; the author allowlist is `trustedBots`, append-only.**
GitHub Apps have **no Discussions permission at all** — the permission does
not appear anywhere in
[GitHub's own permissions reference](https://docs.github.com/en/apps/creating-github-apps/registering-a-github-app/choosing-permissions-for-a-github-app).
The existing `grimoire-index-announce[bot]` App pattern therefore **cannot**
be reused, and any plan assuming it is wrong at the foundation.

- **GitHub, in Actions (the normal case):** `GITHUB_TOKEN` under
  `permissions: { discussions: write }`. Repo-scoped, expires at job end, no
  secret to rotate. Threads are authored by `github-actions[bot]`.
- **GitHub, outside Actions or where `GITHUB_TOKEN` is unavailable:** a
  fine-grained PAT scoped to **one** repository, `discussions: read/write`
  plus the mandatory `metadata: read`, nothing else. A leak lets an attacker
  manage discussions and toggle upvotes in one repo — cannot read code, cannot
  push, cannot touch secrets.
- **GitLab:** a **project access token** (project-scoped bot user, `api`
  scope — no finer award-emoji scope exists). **Never `CI_JOB_TOKEN`:** it
  inherits the triggering user's role
  ([GitLab's own design doc](https://handbook.gitlab.com/handbook/engineering/architecture/design-documents/ci_job_token/)
  names this as a least-privilege violation), which means R-1 clause 2 would
  be satisfied by whichever human triggered the pipeline. That is a direct
  correctness failure of the marker invariant, not merely a hygiene point.

Because the identity in use determines the pinned author id, the id is
**configured, not hardcoded**, and it is a **list**: rotating an identity
*appends*, never replaces. A single-valued field would orphan every existing
thread the moment the credential changed — a nasty one-way door bought off
for the price of a `[]`.

**That list is `index-policy.json`'s existing `trustedBots[].id`, and there is
exactly one of it.** An earlier draft of this ADR also defined a `botIds` key
inside `index.config.json`'s `ratings` block, which R-1 clause 2 then read —
two copies of the same ids, in two files, with no stated sync mechanism. A
rotation touching one and not the other either silently zeroes every vote (the
marker check stops matching) or leaves announce authority stale. The
duplicate is removed: **R-1 clause 2 resolves author ids from
`trustedBots[].id`**, which is already the file whose job is naming trusted
bot identities and already carries the immutable numeric id.
**Correction (2026-08-18, from plan Discover):** an earlier revision said
`findTrustedBot` reads it. It cannot. `findTrustedBot`
(`src/validate/core/ownership.ts:35-47`) is keyed by **login** — its one caller
is the announce gate, which already holds a PR author's login and wants to
confirm the id. R-1 clause 2 needs the **reverse**: given a thread author's
*id*, with no login in hand, is it in `trustedBots[].id`? That is a new
id-keyed scan living in the ratings code, importing only the `TrustedBot`
*type*; `ownership.ts` is expected to be zero-edit. The scan **must skip**
entries in the bare-**string** `TrustedBot` form, which yield `id: undefined` —
treating those as a wildcard would let a login-only entry authorize any
author. Any trusted bot of that index may author a rating
thread; distinguishing the ratings bot from the announce bot buys nothing,
since both are the index operator's own.

The bot's identity is registered in the **existing** `index-policy.json`
`trustedBots` array rather than a second registry, which the `TrustedBot`
object form `{login, id, namespaces}` already accommodates. Least privilege
is preserved by `namespaces: []`: `findTrustedBot` preserves an explicit
empty array (`bot.namespaces ?? ["*"]` fires only on null/undefined) and
`botOwnsNamespace`'s `.some()` over `[]` is `false`, so the ratings bot is
identity-registered with **zero announce authority**. No new config surface —
but, per the correction above, a small new **read** path for the id-keyed scan.

**D6 — Reconcile concurrency is a correctness requirement.**
Neither forge offers an atomic create-if-not-exists, so a push-triggered run
overlapping a scheduled run can double-create a thread for the same ref.
`concurrency: { group: grim-ratings-<repo>, cancel-in-progress: false }`
(Actions) or `resource_group` with `process_mode: oldest_first` (GitLab) is
**mandatory**. `cancel-in-progress: false` matters: the default cancels, and
a queued-not-cancelled run is what makes the level-triggered resync work.

Because a lock cannot prevent every duplicate (a create whose response is
lost after the mutation committed will be retried), the *tally* side is
duplicate-tolerant by R-1's conflict rule. That converts a silent corruption
into a visible operational alarm with both URLs logged for manual deletion.
No de-duplication automation — deleting the wrong thread is unrecoverable,
and a human with two URLs is cheaper than a heuristic.

**D7 — Trigger topology: push primary, schedule secondary, dispatch manual.**
GitHub disables scheduled workflows on public repos after **60 days with no
commit activity**, and *running the scheduled workflow does not reset that
clock*
([GitHub Docs](https://docs.github.com/actions/managing-workflow-runs/disabling-and-enabling-a-workflow)).
So the schedule is least reliable on exactly the quiet indexes that most
need it.

- **Primary: the index build pipeline itself** — push to the default branch,
  merge of an index contribution. This is commit activity, so it never
  silently dies.
- **Secondary: hourly `schedule`** — catches votes arriving with no publish
  activity. Hourly clears GitLab.com's free-tier one-hour floor and sits well
  inside GitHub's five-minute floor with margin for the documented 5–30
  minute dispatch skew.
- **Manual: `workflow_dispatch` / manual pipeline** — the recovery lever
  after a 60-day disable and the way to force a backfill drain.

Consequence, stated rather than hidden: on a quiet index ratings go **stale,
never wrong**. `generated_at` in the file makes that staleness observable —
which is the specific failure the sidecar-staleness research warned about
(the `npm audit` / GHSA join-skew case, where consumers *cannot* detect that
a derived signal has drifted from what it describes).

**D8 — `stats.json` is published, never committed.**
Both platforms support artifact-based Pages deployment with no git commit:
`actions/upload-pages-artifact` + `actions/deploy-pages`, or a GitLab `pages`
job's `artifacts: paths:`. This removes the push→build→commit→push loop by
construction (a Pages deploy is not a push event), removes `[skip ci]`
bookkeeping, and removes a per-vote commit accumulating in history forever.
It also makes *"write only when changed"* moot: regeneration is cheap and the
only reason to gate on change was avoiding a commit, so the file is
regenerated unconditionally.

This matches how the index already treats `all.json`, which is likewise built
into a gitignored `dist/` and deployed rather than committed.

Two consequences. First, `stats.json` has no git history — accepted, the
forge holds the authoritative vote records and their history, and this file
is a derived projection. Second, and sharper: with nothing committed, a
failed tally would deploy a site with **no** `stats.json`, wiping every
displayed rating. That is a data-loss-shaped failure created by the publish
choice, and it is closed by:

> **Invariant R-2 (No Silent Emptying).** The deploy carries forward **each
> stat key** the current run did not successfully produce. A stat key is
> written as empty, and a ref dropped from `entries`, **only** when a
> *completed* producer for that key genuinely observed nothing — never as the
> result of a transport error, an API error, a parse failure, or a skipped run.
> Carry-forward is a **per-key merge over the seed**, not a whole-file
> replacement: a successful `rating` run must not drop a `downloads` key it
> never computed, and vice versa.

Mechanically: the build job seeds `stats.json` from the **live site** before
the fresh artifact is merged over it. The seed *is* the checkpoint — no stored
state, no cache key.

**The seed fetch must distinguish "absent" from "failed", and `|| true` does
not.** A bare `curl … || true` maps a DNS failure, a TLS error, a 500, and a
truncated body to the same outcome as a genuine 404. Combined with a failed
tally that produces no artifact, the deploy would then publish a site with no
`stats.json` at all — the exact wipe R-2 exists to prevent, reached through
the mechanism meant to prevent it. Required behaviour:

| Seed fetch outcome | Meaning | Action |
|---|---|---|
| **404** | No sidecar published yet | Absent is first-class — proceed with an empty seed |
| **2xx, parses** | Previous state | Use as the merge base |
| **2xx, unparseable** | Corrupt or truncated | **Fail the job.** Never treat as empty |
| **Transport error, TLS, 5xx, timeout** | Unknown state | **Fail the job.** Never treat as empty |

So: capture the status explicitly (`curl -sS -o file -w '%{http_code}'`),
branch on it, and let a non-404 failure fail the pipeline rather than deploy a
wiped sidecar. This is the same trap as the indexer's `request()`
(`src/validate/adapters/http.ts:49-70`), which maps every failure to
`{status: 0, body: ""}` — fail-closed for validation, silent-emptying for a
reconciler. A producer must treat `status === 0` as a hard error.

This closes a specific trap: the indexer's `request()`
(`src/validate/adapters/http.ts:49-70`) maps **every** failure to
`{status: 0, body: ""}`. That is a deliberate fail-closed choice for the
validate path and a silent-emptying bug for a reconciler. The ratings
provider must treat `status === 0` as a hard error and exit non-zero.

**D9 — Generated CI stays vendored; volatility lives in the package.**
The operability research recommended a thin stub referencing a versioned
upstream workflow. Rejected, because the premise does not hold here: the
volatile logic is **not in the YAML**. The generated job is checkout,
setup-node, `npm ci`, `grim-indexer ratings`, upload artifact — the
reconcile algorithm, the rate-limit budget, the GraphQL queries, and the
marker parser all live in `@grimoire-rs/indexer`, which downstream indexes
already version-pin and which Dependabot/Renovate already bump.

So a rate-limit tweak or a GraphQL schema fix ships as an indexer version
bump and arrives with no regeneration — the thin stub's entire benefit,
without creating and operating a new versioned workflow repository, without
adding a second distribution model to one generator, and without every
downstream index inheriting a new supply-chain edge. The existing
`verify-ci` drift guard and the `allowManualEdits` escape continue to apply
unchanged.

**Upgrade burden on existing indexes:** bump `@grimoire-rs/indexer`, re-run
`grim-indexer ci` (which they already do for every CI change), add the
`ratings` block to `index.config.json`, register the bot in
`index-policy.json`, and provision the credential. Indexes that do nothing
keep working exactly as today and serve no `stats.json` — which every
client reads as unrated.

**D10 — GraphQL layer placement.**

*grim (Rust):* the send lives beside the existing REST helpers in
`src/catalog/forge.rs` as a `graphql()` function reusing `build_client()` and
`authorize()` verbatim; the query/mutation payloads and response shapes live
in a new `src/catalog/rating_provider.rs`; the command in
`src/command/rate.rs`. Reusing `build_client()` makes the non-regressions
structural rather than promised:
- **Redirects stay `Policy::none()`.** The rationale at `forge.rs:265-273` is
  CVE-class — reqwest otherwise replays `Authorization` / `PRIVATE-TOKEN` onto
  a cross-host `Location`. The rating path carries the *user's own session
  token*, which is broader-scoped than the announce PAT, so the leak would be
  strictly worse. Non-negotiable.
- **`authorize()`'s exhaustive `match` over `ForgeKind` stays exhaustive.**
  Its `Plain` arm sends unauthenticated; a rating **mutation** must instead
  hard-refuse on `Plain`, so this is a new arm, never a wildcard.
- **The `{data, errors}` envelope is new and is the likeliest correctness
  bug.** `get_json` / `send_json` (`forge.rs:1337-1357`) assume a REST shape
  and `status.is_success()`. GraphQL returns **HTTP 200 with a populated
  `errors` array**. `graphql()` must fail on non-empty `errors` independently
  of status, or every failure reads as success.
- The user's token is a `SecretString` with `expose_secret()` called exactly
  once, at the header. It is **not** stuffed into `ForgeContext`, whose
  `token` is a plain `Option<String>` — that field stays as it is.

*indexer (TypeScript):* `request()` gains an optional third parameter
`{method?, body?}` — additive, all eight existing call sites
(`forge.ts:68,85,102,117,136`; `registry.ts:81,100,107`) unchanged. Must not
regress `redirect: "manual"` (`http.ts:59`), and must not inherit the
swallow-everything `catch` for reconciler use (see D8 / R-2). The
`RatingProvider` interface and its two implementations live in
`src/ratings/`, mirroring `src/validate/adapters/forge.ts`'s
`Forge` / `createForge(kind, config)` shape.

**Asymmetry, recorded deliberately so reviewers do not read it as an
oversight:** the indexer gets a real `RatingProvider` **interface** with two
implementations plus an in-memory fake; grim gets a `match` on a string and
**no trait**. This is not inconsistency. The indexer needs the interface for
three independent reasons — two live implementations, a third in-memory
implementation that is the primary local test seam, and a `createForge`-shaped
factory that already exists as precedent. grim has a closed two-arm set, one
binary crate, no library API, and
[`arch-principles.md`](../../.claude/rules/arch-principles.md)'s standing rule
that internal enums stay matchable precisely so a new arm cannot compile
without being handled. A trait there would be an interface with one call site
and two implementations that never vary independently — YAGNI, and it would
hide the exhaustiveness the codebase relies on.

**D11 — Null policy per surface.**

| Surface | Policy | Rule and precedent |
|---|---|---|
| `stats.json` (published artifact) | **Omit** | Zero-vote entries absent; `entries` key absent when there is nothing to say. Governed by the OSV-style consumer sentence in the schema contract below |
| `CatalogEntry.rating` (grim's on-disk cache) | **Omit** | `#[serde(default, skip_serializing_if = "Option::is_none")]`, matching every other optional field on that struct (`deprecated`, `replaced_by`, `revision`, …) |
| `--format json` reports | **Always-present-null** | `skip_serializing_if` is **banned** in `src/api/` (`subsystem-cli-api.md`). `SearchEntry`'s hand-written `Serialize` goes from 13 fields to 14, and `json_carries_replaced_by_plain_table_does_not` — which asserts the count — is updated in the same commit |
| `CatalogPackage.rating` (site) | **Omit** | TypeScript optional; matches the Astro site's consistent omit-don't-placeholder idiom (`Catalog.tsx` `meta-row`) |

Consumer contract text is added to `docs/src/json-interface.md` beside the
existing always-present-null paragraph (`:514-520`).

**D12 — Client-side vote state is tri-state and local; the aggregate never
carries it.**
`stats.json` is one anonymous document served identically to every
consumer and cached, so "has *this user* voted" is by construction absent
from it. That question is answered per surface:

- **The static site never knows.** It is prerendered with no identity at
  build time and no runtime fetch (D1), so its vote affordance is always
  neutral. This is a consequence of D1 that is easy to discover late; it is
  recorded here rather than in a UI ticket.
- **After a click, the mutation response is authoritative.** GitLab's
  `awardEmojiToggle` returns `toggledOn`; GitHub's upvote mutations return
  the subject carrying `upvoteCount` **and**
  [`viewerHasUpvoted`](https://docs.github.com/en/graphql/reference/discussions).
  No second query, no optimistic guess that can disagree with the forge.
- **Before a click, on a fresh machine**, the answer requires a token and a
  network call — `viewerHasUpvoted` on GitHub, or locating the user's own
  award in GitLab's emoji list.

`grim rate` therefore records its own result in `$GRIM_HOME/state/votes.json`,
keyed by **(ref, forge identity)** — never by ref alone, or two accounts on
one machine display each other's votes, and a rotated credential inherits the
previous user's state. `viewerHasUpvoted` is a **lazy refinement on a detail
view only**, when a token is already in hand; it is never a bulk query across
the catalog, which would authenticate and de-staticise the read path.

> **Invariant R-3 (Tri-State Vote Display).** A client renders one of
> **voted / not-voted / unknown**, never a boolean. An absent or unreadable
> local record means **unknown**, and unknown renders neutral — never
> "not voted". Collapsing unknown into not-voted is what makes a user
> confidently re-cast a vote they already hold.

`votes.json` is a cache, not state: absent is first-class, a corrupt or
unparseable file is discarded and treated as unknown rather than raising, and
nothing else reads it. It is deliberately outside the install-state schema
(`state.json`, V2) so it inherits no migration obligation.

```jsonc
{
  "schema_version": 1,
  // Key: "<forge-identity>\u0000<ref>". Forge identity is the provider's own
  // immutable numeric account id (GitHub `viewer.databaseId`, GitLab
  // `currentUser.id`), NEVER the login — the same rename-proof rule R-1
  // clause 2 applies. Unknown identity ⇒ do not write; render unknown.
  "votes": {
    "github:41898282\u0000ghcr.io/grimoire-rs/grim-usage": {
      "voted": true,
      // RFC3339. From the mutation response, not the local clock's opinion
      // of when it asked.
      "observed_at": "2026-08-18T09:00:00Z"
    }
  }
}
```

**Write and precedence rules, so a stale "voted" cannot persist:**

1. **Written only after a successful mutation response**, from that response's
   own `viewerHasUpvoted` / `toggledOn`. Never optimistically before the call,
   never from a failed or timed-out call — a timeout leaves the entry
   *untouched*, which reads as unknown, not as voted.
2. **`viewerHasUpvoted` always wins.** Whenever a detail view fetches it, its
   value overwrites the record. The forge is authoritative; the cache is a
   convenience.
3. **The record expires.** An entry older than the catalog TTL window is
   treated as **unknown**, not as its last value. This is what stops a vote
   retracted through the forge web UI, or cast on another machine, from
   displaying wrong indefinitely — R-3 already requires unknown to render
   neutral, so expiry degrades safely by construction.
4. **A count that disagrees is not a signal.** `stats.json`'s `up` is an
   anonymous aggregate and can never confirm or refute *this user's* state;
   only `viewerHasUpvoted` may overwrite the record.

**D13 — The forge endpoint is resolved by host-matched credential lookup;
`--token-stdin` inherits that discipline one level up.**
An earlier review raised index-supplied host data as a token-exfiltration
path. It is not, for grim's own resolution: the existing ladder resolves a
credential **for the host it is about to contact** (`forge.rs:37-96`,
`:150-180`), so an index naming an attacker host simply resolves no
credential and the request goes out bare or fails — the same property that
already protects `--announce`.

The one path that bypasses it is `--token-stdin`, where the extension
*injects* a credential rather than grim resolving one. The rule is therefore
placed on the injector: **the extension selects its authentication provider
from the same host it will hand to grim, and pipes nothing when that host is
not one it authenticated against.** Host-matching applied one level up, not a
new mechanism.

The GraphQL client is built through the existing `build_client()`
(`forge.rs:263-278`) and inherits its hard-disabled redirects — the
documented CVE-class defence against replaying an `Authorization` header
cross-host. That inheritance is a requirement, not an incidental.

**Host comparison is exact, not fuzzy.** Ports included, ASCII-lowercased,
IDNA-normalised, no suffix matching — `evil-github.com` and
`github.com.evil.tld` must not match `github.com`, and the comparison reuses
whatever the announce ladder already does rather than inventing a second
notion of host equality. The provider→host mapping is `github` ⇒
`api.github.com` and `gitlab` ⇒ `gitlab.com` by default, overridable **only**
from the user's own config for GHES and self-managed instances — never from
index-fetched content.

**`--token-stdin` edge cases, specified rather than discovered:**

| Condition | Behaviour |
|---|---|
| stdin is a TTY | **Refuse**, exit `64` — the flag exists for piping; prompting here would invite a paste into scrollback |
| stdin empty or whitespace-only | **Refuse**, exit `80` (no credential resolvable). Never fall through to the standalone ladder — the caller *stated* it was supplying a credential, so silently using a different one is worse than failing |
| More than one line | Trailing newline stripped; any further content is a usage error, exit `64` |
| Mutation fails after the token was read | Token is dropped (`Zeroizing`), `votes.json` is **not** written, exit per the forge's own failure class (`69`/`80`). No partial record |
| Token appears in output | Never. Not in errors, not in `--format json`, not in a panic message — `SecretString`, `expose_secret()` once at the `Authorization` header |

**D14 — `stats.json`, not `ratings.json`; ratings are one statistic of
several, and sorting is in scope.**

*On the name.* `stats` is chosen for recognition, not novelty. The closest
prior art is the VS Code Marketplace Gallery API, whose `statistics` array
carries `install`, `averagerating`, `ratingcount`, `weightedRating`, and
`downloadCount` **in one bag** — the exact shape this file needs, from the
ecosystem nearest to Grimoire's (Open VSX was Option 3 above). `vsce show`
reads it; the field is undocumented but stable and widely consumed
([vscode-vsce#847](https://github.com/microsoft/vscode-vsce/issues/847)).
Beyond that, `stats` is the term already in every neighbouring tool —
`pypistats`, `npm-stat`, GitHub's Insights → Traffic **stats** — so a reader
meeting `<base>/stats.json` for the first time does not need to be taught what
it holds.

Three statistics answer three different questions about an artifact: **ratings**
(is it good?), **downloads** (is it used? — [#89](https://github.com/grimoire-rs/grimoire/issues/89)),
and **recency** (is it maintained?). Only the first needs this ADR's
machinery; the third is already free (`created`, off the OCI manifest
annotation); the second is blocked on per-backend APIs the default registry
does not offer.

The published sidecar is therefore named for the **family**, not for the one
member shipping first, and each ref maps to a bag of stats rather than to a
flat rating. This is a naming decision made *now* because it is the one part
that is genuinely one-way: the published locator joins `stability.md`'s
"Package index transport" row and is frozen at 1.0. Renaming today costs a
find-and-replace in an unlanded document; renaming after v1 means serving a
legacy `ratings.json` forever. Adding `downloads` later then costs one
additive key inside an existing entry — no second file, no second fetch, no
second cache, no second freshness model.

**What deliberately does *not* get renamed:** `RatingSummary`,
`CatalogEntry.rating`, `SearchEntry.rating`, `CatalogPackage.rating`, and
`grim rate`. Each names the *rating* signal specifically and stays accurate
when a sibling arrives; the report fields are flat and additive, matching
`created`, which is already a flat sibling. Only the frozen locator and wire
shape needed the generalisation — grouping the Rust and report surfaces too
would be speculative structure for a statistic that does not exist yet.

**One consequence to specify rather than discover:** R-2's seed-from-live-site
carries the *whole* file forward. With multiple producers, a run where ratings
succeeded and a future downloads producer failed must merge **per stat key**,
not overwrite wholesale — otherwise a fresh rating write silently drops
last-run downloads. R-2 is restated accordingly: *carry forward each stat key
the current run did not produce.*

**Sorting is in scope, not display-only.** The site's `Sort` union widens to
`"name" | "updated" | "rating"`, with the same option in the TUI and a
`grim search --sort` flag; `"downloads"` joins additively if #89 lands. Order
is **rating desc, then updated desc, then name** — deterministic, and unrated
artifacts fall through to recency, which is the signal that is always present.
Absent ratings sort *last*, never as zero, so a brand-new artifact is not
buried beneath a single upvote.

### Backwards compatibility, per artifact

Principle 9 is the owner's top priority, so this is enumerated rather than
asserted.

| Artifact | What changes | What must not break | Old client ← new data | New client ← old data |
|---|---|---|---|---|
| **`all.json`** | **Nothing. Byte-identical.** Guaranteed structurally: `compileIndex` (`src/data/index.ts:121-167`) is not modified at all; the join happens downstream in `buildSite` | The frozen `<base>/all.json` transport locator and record shape (`stability.md` "Package index transport") | Unaffected | Unaffected |
| **`stats.json`** | New file. v1 defines the contract; from v1 on, additive-only under a monotonic integer `schema_version` | Never change what a field can contain — add a field instead (the crates.io `features2` rule) | A pre-ratings grim never requests it | No file ⇒ 404 ⇒ no ratings; catalog builds normally |
| **`index.config.json`** | New optional top-level `ratings` block | Every existing key; `SiteConfig` is coerced field-by-field, not schema-validated, so unknown keys are ignored rather than rejected | An older indexer ignores the block; ratings simply do not run | No block ⇒ ratings off |
| **`index-policy.json`** | One appended `trustedBots` entry, using the already-supported `{login, id, namespaces}` object form with `namespaces: []` | The `TrustedBot` union and `findTrustedBot`/`botOwnsNamespace` semantics — unchanged, no code edit | An older indexer parses the entry and grants it nothing (empty namespaces) | Unaffected |
| **`CatalogEntry`** (cache) | New optional `rating`, `skip_serializing_if = "Option::is_none"` so an unrated entry is byte-unchanged | Cache format is not a stability contract, but the struct carries `deny_unknown_fields` | **An older grim rejects the newer cache and rebuilds it — true from 0.14 on, and it was NOT true before.** See the correction below | `#[serde(default)]` ⇒ `None` |
| **`--format json` reports** | Additive always-present-null `rating` on `SearchEntry`; a new single-object `grim rate` report | Frozen report shapes; the additive-field policy and its reader obligation (`stability.md#frozen-additive-fields`) | A consumer that ignores unknown keys is unaffected — the obligation is already documented | Emitted unconditionally |
| **CLI surface** | New `grim rate` subcommand | Nothing removed or renamed; the CLI surface is additive-only | An older grim exits **64** on `rate`; `grimoire-vscode` adds a **new `RATING_GRIM_VERSION` gate** and leaves `MINIMUM_GRIM_VERSION` (`'0.11.0'`) alone — see the correction under the Implementation Plan | — |
| **VS Code extension** | New vote affordance, gated | The `execFile`-only, `--format json`, no-shell contract; the extension constructs **no** forge URLs and shells out for everything | An extension version predating the feature shows no vote UI | An extension with the feature against an older grim: the version gate hides the affordance |

> **Correction (WP-A, execution).** The "rejects and rebuilds" claim above,
> and `registry_catalog.rs`'s own doc comment, were **false about the code as
> shipped**. `Catalog::load` returned `Err(Parse)` on a rejected cache, and
> that read happened *before* the rebuild decision in both `load_or_refresh`
> and `coordinate`, so the rebuild never ran: `load_catalog` caught the error,
> logged `catalog for source '…' unavailable`, and degraded that registry to an
> empty group **without overwriting the cache file**. The source then browsed
> empty on every subsequent run — `--refresh` included — until a human deleted
> the file. A permanent wedge, not one refresh. WP-A fixed the code rather than
> softening the claim: `Catalog::load_or_cold` treats an online load failure as
> a **cold** cache (warned, naming the file) so the rebuild overwrites it, while
> offline the error still propagates, because there is nothing to rebuild from
> and reporting the registry as empty would be a lie. `Catalog::load` itself is
> unchanged, so its documented strictness and `unknown_version_rejected` still
> hold — only the *fatality* of a rejection changed, not the rejection.
>
> **This cannot help the already-released 0.13 binary.** S-015 as written
> ("run 0.14, downgrade to 0.13 ⇒ one network refresh") is unachievable from
> this repository; the fix makes it true from 0.14 onward. The 0.13 downgrade
> path needs a release note telling users to delete `$GRIM_HOME/catalog/`,
> owned by WP-X. `skip_serializing_if` narrows the blast radius meanwhile: a
> cache whose entries are all unrated is byte-identical to a 0.13-written one,
> so only a user browsing a *rating-publishing* index is exposed at all.

The `deny_unknown_fields` row deserves its own sentence because it is the one
place this decision makes a downgrade *worse*: a user who runs 0.14 then
downgrades to 0.13 pays one full catalog rebuild. That is the documented,
already-accepted trade on this struct, and it costs one network refresh, not
data.

### Scale decision, with measurable expiry

Forge-as-database is comfortable to roughly **1–2k artifacts**. The catalog
holds ~200 today. The binding constraint is **thread creation**, not tallying
or serving: GitHub's secondary limit is **80 content-creating requests per
minute and 500 per hour**, so a 10k cold start is ~20 hours of wall clock —
which a fixed per-run creation budget (400/run) converts into ~25 unattended
scheduled runs with no coordination code, because the reconciler is stateless
and idempotent.

Revisit triggers, each measurable from the one structured log line the job
already emits:

1. **Backfill cannot drain.** `created=X/Y` shows `X < Y` for **three
   consecutive scheduled runs** — the creation rate is losing to the publish
   rate, and the ceiling is real rather than transient.
2. **`stats.json` exceeds ~200 KB gzipped** after zero-vote omission
   (~75 KB projected at 10k artifacts with 5% participation; 200 KB is
   roughly 25–30k rated entries). At that point it is no longer a cheap
   sidecar and wants pagination or a query API.
3. **A requirement the forge cannot express**: weighted score, review text,
   per-version ratings, or a retraction audit. Note that *whether the current
   user has already voted* is **not** on this list — both forges express it
   (`viewerHasUpvoted`; the GitLab award list), it simply costs a token and a
   call, which is why D12 makes it a lazy per-view refinement.

4. **Engagement never materialises.** Triggers 1–3 all detect the feature
   *succeeding* at scale. This one detects the opposite, and it is the trigger
   the decision margin actually demands: **five consecutive tallies in which
   fewer than 5% of published refs carry any vote**, measured after the
   backfill has fully drained (so it cannot fire on a half-created index).
   The chosen option beats doing nothing by 3 points of 85, and that margin
   rests entirely on the fitness column — which assumes people vote. If they
   do not, the assumption behind the decision is falsified and this ADR should
   be reopened, not quietly carried. The tally job already emits `refs=N` and
   `tallied=Z`; the ratio is free.

**Migration cost when a trigger fires.** The data seam is already the
contract, so the mechanical cost is small: stand up a service that emits the
same `stats.json` under the same `schema_version`, add one `match` arm in
grim and one `RatingProvider` implementation in the indexer. The site changes
nothing. `all.json` changes nothing. Existing `stats.json` consumers keep
working through the transition.

**The unmigratable part is vote history.** Votes live on the forge as upvotes
bound to forge accounts, and no export preserves one-vote-per-human across
the boundary. Migration therefore either restarts from zero or seeds *counts*
(never identities) from the last tally and accepts that a user could vote once
on each side. This is the genuine one-way cost of the decision and is easy to
hand-wave; it is recorded here so a future migration prices it up front.

### Non-functional requirements

| NFR | Effect of this decision |
|---|---|
| **Scalability** | Bounded by forge thread creation (80/min, 500/hr), not by tally or serving; comfortable to ~1–2k artifacts, with the three measurable triggers above marking the boundary |
| **Availability** | Browsing never depends on a service this project operates: reads are a static file behind Pages/CDN and work offline from grim's cache. The write path is optional and its failure is a refused vote, never a degraded browse |
| **Latency** | Read latency is unchanged for the site (baked into the bundle) and one additional cached HTTP GET per index refresh for grim — at most hourly per source. A vote is a single round trip. Rating *freshness* is bounded by the tally cadence (hourly), which is a deliberate trade, not a latency defect |
| **Security** | One new Block-tier surface (R-1 marker authority) and one new credential path (user session token via `--token-stdin`, `SecretString`, `expose_secret()` once). No signed aggregate, no new OAuth application, no custom anti-abuse system — all three disproportionate to a deliberately unsigned, deliberately service-free system. GitHub's missing self-upvote block is documented, not engineered against |
| **Cost** | Effectively zero at every scale considered. Public repos run Actions free; GitHub-hosted Linux is $0.006/min for private ones and the job is sub-minute; GitLab self-managed has no CI-minute billing. `stats.json` bandwidth is trivial. Zero marginal human maintenance beyond watching for a red X |
| **Operability** | Stateless, level-triggered reconciler: list → diff → budgeted-create → tally → publish. A partial run leaves *fewer threads created*, never corrupt state, and the next run's list-and-diff continues. One structured log line per run (`refs=N created=X/Y tallied=Z changed=<bool> secondary_limit_hit=<bool>`) answers every operational question without a dashboard. Fail loud on anything that is not a documented rate-limit backoff — for a `schedule` trigger the red X is the only alert that exists |

### Consequences

**Positive:**
- A quality signal ships with no service, no database, no OAuth application,
  and no new uptime dependency — for the public index and every self-hosted one.
- Identity, moderation, abuse reporting, and rate limiting are inherited from
  the forge rather than built.
- `all.json` is untouched, structurally: the function that writes it is not
  modified.
- The provider is a data seam, so replacing the forge with a service later
  costs one `match` arm and one implementation, with no client change.
- The write path's failure modes are all *no vote recorded* — never a
  degraded browse, never a corrupted read.

**Negative:**
- Rating freshness is bounded by the tally cadence; a vote is invisible for up
  to an hour.
- Ratings require GitHub Discussions (or GitLab work items) enabled plus an
  announcement-format category — a manual prerequisite the tooling cannot
  create for the operator.
- On GHES, Discussions requires 3.6+; older instances cannot participate.
- GitHub has no native self-upvote block, so an author can boost their own
  artifact there. GitLab refuses self-awards natively; the asymmetry is
  documented rather than engineered against — building self-vote detection is
  exactly the speculative complexity the thin decision margin forbids.
- **Coordinated Sybil inflation** — multiple accounts upvoting one artifact —
  is likewise accepted and named rather than defended against. It is cheap on
  a public forge (free accounts, no permission barrier to upvote a public
  discussion) and demonstrated adjacent to this exact use case: researchers
  reached VS Code Marketplace "trending" status with fabricated reviews in
  about thirty minutes
  ([writeup](https://medium.com/extensiontotal/the-story-of-extensiontotal-how-we-hacked-the-vscode-marketplace-5c6e66a0e9d7)).
  It is a trust-erosion attack, not a code-execution one — inflating a count
  grants no install authority, unlike tampering with the aggregate, which R-1
  covers. The forge's own account-age and abuse tooling is the whole defence.
- A full site rebuild and redeploy runs hourly on the schedule even when
  nothing changed.
- Votes cannot be migrated to a future backend; only counts can.

**Risks:**
- *Marker forgery redirects organic votes onto a malicious package.* Mitigated
  by R-1 (author-id-scoped, top-level-only matching) — Block-tier, must be
  built as specified and tested before any write path lands.
- *A failed tally wipes every displayed rating.* Created by the
  publish-don't-commit choice; mitigated by R-2 (seed from the live site, and
  never write an empty tally on error).
- *Duplicate thread creation from concurrent runs.* Mitigated by D6's
  mandatory concurrency lock, and made **visible** rather than silent by R-1's
  conflict rule.
- *`CI_JOB_TOKEN` used on GitLab out of convenience*, silently satisfying R-1
  clause 2 with a human's identity. Mitigated by naming it as a correctness
  failure in D5 and refusing it explicitly in the provider.
- *Scheduled workflow silently disabled after 60 days on a quiet public
  index.* Accepted and documented; the push-triggered path is primary for
  exactly this reason, and the failure mode is stale, not wrong.

## Technical Details

### Architecture

```
  ┌──────────────┐    grim publish --announce    ┌────────────────────┐
  │  publisher   │ ───────────────────────────►  │  index repository  │
  └──────────────┘                               │  index/**/*.json   │
                                                 └─────────┬──────────┘
                                                           │ CI: build pipeline
                                    ┌──────────────────────┴───────────────────┐
                                    │ 1. grim-indexer ratings                  │
   ┌──────────────┐   GraphQL       │      reconcile (create missing threads)  │
   │  FORGE       │ ◄───────────────┤      tally (read upvote counters)        │
   │  Discussions │                 │      → .stats.json                     │
   │  / work items│                 │ 2. grim-indexer build                    │
   └──────┬───────┘                 │      compileIndex → all.json (untouched) │
          │                         │      buildSite    → stats.json + bundle│
          │  addUpvote /            │ 3. deploy-pages (artifact, no commit)    │
          │  awardEmojiToggle       └──────────────────────┬───────────────────┘
          │                                                │ static
   ┌──────┴────────┐                          ┌────────────▼─────────────┐
   │  grim rate    │                          │  <base>/all.json         │
   │  (user token) │                          │  <base>/stats.json     │
   └──────┬────────┘                          └────────────┬─────────────┘
          │ --token-stdin                    join by ref   │
   ┌──────┴────────┐                          ┌────────────▼─────────────┐
   │ grimoire-     │  execFile --format json  │ grim search / TUI / MCP  │
   │ vscode        │ ───────────────────────► │ CatalogEntry.rating      │
   └───────────────┘                          └──────────────────────────┘
```

### API Contract

**`stats.json`** — served at `<base>/stats.json`, sibling of `all.json`.

```jsonc
{
  // Monotonic integer, crates.io style — NOT semver. Principle 9 removes the
  // "minor vs major" axis semver exists to communicate.
  "schema_version": 1,

  // RFC3339 UTC. Makes staleness observable to every consumer — the one thing
  // OSV/GHSA sidecar consumers cannot do (see the npm-audit join-skew case).
  "generated_at": "2026-08-18T09:00:00Z",

  // Per-STAT provider block. Each statistic names its own producer,
  // because they are genuinely different: ratings come from a forge, a future
  // `downloads` would come from the registry (#89). A single top-level
  // `provider` would have been wrong the moment a second signal landed.
  "providers": {
    // Which write mutation `grim rate` issues. A plain string, NOT a tagged
    // union: `target` and `url` are hoisted onto the entry (D3), so this
    // carries no read-path data and an unrecognised value degrades to
    // "readable, not writable".
    "rating": "github"
  },

  // Keyed by artifact ref, exactly as it appears in all.json's `ref`.
  // Each ref maps to a bag of SIGNALS. A stat with nothing to say is
  // OMITTED; a ref with no stats at all is omitted; the whole `entries`
  // key may be absent. Absent is first-class at all four levels.
  "entries": {
    "ghcr.io/grimoire-rs/grim-usage": {
      "rating": {
        "up": 42,
        // Opaque. No client parses or constructs it. The forge node id the
        // vote mutation targets.
        "target": "D_kwDOABCDEF4AQtBz",
        // Opaque. The human-facing thread link grim hands to the extension.
        "url": "https://github.com/grimoire-rs/index/discussions/117"
      }
      // A future "downloads" sibling lands here as a purely additive key
      // (#89). Recency deliberately does NOT live here — `created` already
      // rides the OCI manifest annotation and costs nothing.
    }
  }
}
```

**Consumer contract**, stated verbatim in `docs/src/package-index.md` (the
OSV formulation, which is the clearest in the ecosystem):

> A client that understands `schema_version` *N* must accept and correctly
> process any document declaring `schema_version` ≤ *N*, ignoring fields it
> does not recognize. A document declaring `schema_version` > *N* may be
> skipped or degraded to "no rating available", but must never be treated as
> a parse error. A missing file, a missing entry, and a missing field all mean
> **unrated** — never an error, at any of the three levels.

**`index.config.json`** — new optional top-level block:

```jsonc
{
  "ratings": {
    "provider": "github",              // "github" | "gitlab"
    // GitHub: Discussion category (announcement format recommended).
    // GitLab: work item type, "Issue" or "Task".
    "container": "Ratings",
    // NOTE: there is deliberately NO botIds key here. R-1 clause 2 reads the
    // author allowlist from index-policy.json's existing `trustedBots[].id`
    // (see D5) — a second copy of the same ids in a second file is a
    // consistency hazard, not a convenience.
    // Per-run thread-creation budget, under GitHub's 500/hour secondary cap.
    "createBudget": 400,
    // Lock threads on creation: votes still count, replies are refused.
    // Default TRUE. An index operator who wants a rating signal without
    // running a comment forum they must moderate gets that by default, and
    // it independently HARDENS R-1 — a locked thread cannot receive the
    // forged-marker reply that clause 1 exists to reject. Set false only to
    // deliberately run discussion alongside voting.
    "lockThreads": true
  }
}
```

**`grim rate`** — new subcommand.

```
grim rate <ref> [--up | --remove] [--yes] [--token-stdin] [--registry <ref>] [--format json]
```

| Aspect | Contract |
|---|---|
| `<ref>` | An artifact reference resolved through the same seam `grim search` uses; must resolve to exactly one catalog row carrying a `rating` with a `target` |
| `--up` | Default. Registers an upvote (`addUpvote` / `awardEmojiToggle` on) |
| `--remove` | Retracts *this user's own* upvote. **Not a downvote** — votes stay up-only and binary; both forges' primitives are toggles, so retraction is the same API surface and its absence would leave a mis-click unrecoverable. Stated as an assumption on the gate's "up-only and binary" ruling, not a widening of it |
| `--yes` | Skips the confirmation. **Voting confirms by default** (owner ruling, 2026-08-18): a vote posts publicly under the user's own forge account, so an interactive run prompts `This posts publicly to your <provider> account as <login>. Continue? [y/N]` and a decline exits `0` with no mutation. **Non-interactive without `--yes` exits `64`**, naming the flag — never a hang, never an unconfirmed vote. **`--token-stdin` implies non-interactive and therefore requires `--yes`**: stdin is carrying the credential, so it cannot also carry a `y`, and the prompt is deliberately *not* rerouted to `/dev/tty` — a credential-piping caller is a program, and programs confirm with a flag. `grimoire-vscode` always passes it, carrying its own disclosure |
| `--token-stdin` | Reads the credential from stdin, mirroring `grim login --password-stdin` (`login.rs:45`, read path `:156-176`). `Zeroizing<String>` → `SecretString`, `expose_secret()` once at the `Authorization` header. **No `--token VALUE` flag** — argv lands in world-readable `/proc/<pid>/cmdline` (CWE-214) |
| Credential ladder (standalone) | host-matched env var → `gh`/`glab` stored credential → read-only refusal. Mirrors the existing announce ladder's shape (`forge.rs:37-96`, `:150-180`), with its **own, narrower** credential — it does not reuse the announce token. **Correction (WP-D, execution): the device-flow rung is struck.** A device flow needs a registered OAuth client id, and this ADR's own Security section rules out registering an OAuth app — the two clauses contradicted each other. The ladder ends one rung earlier, at the read-only refusal, which is the behaviour the exit-`80` contract already described |
| Report | Single-object (`release_report.rs` shape), always-present-null fields: `{ref, action, up, url, provider}` |

Exit codes, from `src/cli/exit_code.rs`:

| Code | Condition |
|---|---|
| `0` | Vote registered or retracted |
| `64` `UsageError` | `--up` and `--remove` together; malformed ref; **non-interactive without `--yes`**; `--token-stdin` without `--yes` |
| `65` `DataError` | Catalog row has no `rating`; `provider` unrecognised (`UnsupportedProvider`, raw value in the message); GraphQL `errors` populated |
| `69` `Unavailable` | Forge unreachable, 5xx, or secondary rate limit |
| `79` `NotFound` | Ref resolves to no catalog row, or the thread `target` no longer exists |
| `80` `AuthError` | 401/403 from the forge, or no credential resolvable |
| `81` `OfflineBlocked` | `--offline` / `GRIM_OFFLINE` — a vote hard-refuses rather than degrading silently |

**`RatingProvider`** (indexer, TypeScript) — mirrors the existing
`Forge`/`createForge` shape at `src/validate/adapters/forge.ts:20-28,146-151`:

```ts
export interface RatingThread {
  ref: string;        // parsed from the authorized marker
  target: string;     // opaque forge node id
  url: string;        // human link
  up: number;         // native upvote counter
}

export interface RatingProvider {
  /** Every thread the bot authored in the configured container, paginated
   *  to exhaustion. R-1 filtering (top-level only, author id, first marker)
   *  is applied here — never by the caller. */
  listAuthored(): Promise<RatingThread[]>;
  /** Create one thread carrying `<!-- grim-ref: <ref> -->`. Callers respect
   *  the per-run budget; the provider does not enforce it. */
  create(ref: string): Promise<RatingThread>;
}

export function createRatingProvider(
  kind: "github" | "gitlab",
  config: RatingProviderConfig,
): RatingProvider;
```

**Rust provider dispatch** — deliberately not a trait (D10):

```rust
// src/command/rate.rs — the single dispatch site.
match provider.as_str() {
    "github" => rating_provider::github_vote(&client, &ctx, target, action).await,
    "gitlab" => rating_provider::gitlab_vote(&client, &ctx, target, action).await,
    other => Err(RateError::UnsupportedProvider(other.to_string())),
}
```

### Data Model

`CatalogEntry` gains one field, following `deprecated` / `replaced_by`
verbatim:

```rust
/// Community rating, joined from the index's `stats.json` sidecar at
/// catalog-build time. `None` when the index publishes no sidecar, the
/// artifact has no entry in it, or the source is not an HTTP index.
/// Absence means "unrated", never an error.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub rating: Option<RatingSummary>,
```

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RatingSummary {
    /// Upvote count at the sidecar's `generated_at`.
    pub up: u32,
    /// Opaque vote-subject handle. Never parsed or constructed by grim.
    pub target: String,
    /// Opaque human-facing thread link. Never parsed or constructed by grim.
    pub url: String,
}
```

Note the `deny_unknown_fields` on `RatingSummary`: it is the *cache*
representation, written and read only by grim, so it follows `CatalogEntry`'s
existing discipline. The **wire** representation in `stats.json` is parsed
by a separate lenient struct with no `deny_unknown_fields`, which is where
forward compatibility with a future `schema_version` actually lives. Do not
collapse the two — that is precisely the
[serde `deny_unknown_fields` forward-compat trap](https://github.com/serde-rs/serde/issues/2634)
the compatibility research flagged.

## Implementation Plan

Sequencing is by **contract**, not by repository — each step lands a contract
the next depends on, and each repo can ship independently once its inbound
contract exists.

1. [ ] **`stats.json` schema + fixtures** land first, in the indexer, with
       no producer. Four fixtures per the compatibility research: minimal
       valid v1; v1 with unknown top-level keys; unknown `provider`; a ref
       absent from `entries`. This is the contract both other repos build
       against, and it is the only step with no prerequisite.
2. [ ] **grim read path** — `IndexPackage`/`fetch_index_entries` sidecar
       fetch, `CatalogEntry.rating`, `SearchEntry.rating` (13 → 14 fields, test
       updated), TUI detail-pane row. Deserializes the same fixtures.
       *Ships independently and is useful alone:* an index that already
       publishes ratings shows them.
3. [ ] **Indexer reconcile + tally** — `RatingProvider`, the in-memory fake,
       R-1 enforcement with its four-case test, budgeted creation, the
       `grim-indexer ratings` verb, `request()`'s additive `{method, body}`.
4. [ ] **Indexer CI generation + site join** — `ratings` config block, the
       generated job with its concurrency lock and seed-from-live step,
       `buildSite`'s ref join, `CatalogPackage.rating`, the card meta-row.
5. [ ] **grim write path** — `grim rate`, `--token-stdin`, the credential
       ladder, `graphql()` with `{data, errors}` handling, `rate_report.rs`.
       Depends on step 2 for the `target` it votes against.
6. [ ] **VS Code extension** — a **new `RATING_GRIM_VERSION` constant**
       (**not** a `MINIMUM_GRIM_VERSION` bump — correction 2026-08-18: that
       constant is the hard floor gating *every* existing command, so raising
       it would lock users out of the whole extension over an optional feature.
       The repo already has the right pattern: `REGISTRY_EDIT_GRIM_VERSION`
       (`installer.ts:32`) beside `MINIMUM_GRIM_VERSION` (`:26`), each with its
       own capability check), `rateArgs()`
       builder, session-token piping via `child.stdin`, the vote affordance,
       and the "voting posts publicly to your account" disclosure. Depends on
       a released grim carrying step 5.
7. [ ] **Docs** — `package-index.md` (the `stats.json` spec + consumer
       contract), `commands.md` (`grim rate`), `json-interface.md` (the
       `rating` field), **`hosting-an-index.md`** (the `ratings` block, added
       to the existing `index.config.json` key table, plus the generated
       ratings job) — **not `configuration.md`, which documents grim's own
       `grimoire.toml`, not the indexer's config** — `stability.md` (whether
       `stats.json` joins the existing "Package index transport" row or gets
       its own, plus the `CatalogEntry` downgrade note and a `SearchEntry.rating`
       entry in the Additive-fields worked examples), and a one-line pointer
       from `authentication.md` to `grim rate`'s separate credential ladder.
8. [ ] **AI config + first-party catalog drift** — a `grim rate` row in
       `.claude/rules/subsystem-cli-commands.md` (no structural test catches
       its absence, so it drifts silently), and the catalog-drift duty that
       `AGENTS.md` makes unconditional for `src/command/**`: `grim-usage`
       needs `rate` in both its `description` frontmatter (the CSO discovery
       trigger) and its Command Map table; `grim-authoring` needs an explicit
       reviewed-no-change disposition. `task catalog:verify` gates this.

## Validation

- [ ] Four `stats.json` fixtures deserialize through the **real** Rust
      structs and the **real** TypeScript parser — not a JSON-Schema check,
      which validates shape but says nothing about what a parser does with
      unrecognised bytes.
- [ ] R-1's four-case test passes against the in-memory provider fake
      (stranger reply → 0, stranger top-level → 0, bot top-level → counted,
      duplicate bot posts → 0 + warning).
- [ ] R-2 verified: a tally that errors mid-pass writes no `stats.json`, and
      the deploy carries the live copy forward.
- [ ] `all.json` byte-identical across the change — asserted by a golden
      fixture, not by inspection.
- [ ] An index with no `stats.json` produces zero warnings and a clean
      catalog build in grim, offline and online.
- [ ] `grim rate --offline` exits 81; no credential ever appears in argv, in
      any log line, or in any `Debug`/`Serialize` output.
- [ ] Security review of the write path and R-1 before merge
      (`/security-auditor`, per `.agents/memory/hex.md`'s always-on
      credential-path perspective).

## Open Questions

All three markers were resolved by the owner on 2026-08-18. Kept here with
their resolutions rather than deleted, so the reasoning survives.

- [x] **Public-index rollout — RESOLVED: enable at v1.** `index.grimoire.rs`
      is this project's own index and dogfooding the feature is the point.
      Discussions get enabled on `grimoire-rs/index` and ratings ship on.
      **The credential and the live enablement are owner actions** — no
      implementation step touches the production index, its settings, or its
      secrets without an explicit go-ahead at that moment. Plan approval is
      not consent to write to a live public surface.
- [x] **`grimoire-lore` — RESOLVED: out of scope, permanently.** It belongs to
      OCX and is a *consumer* of this project, not part of it. v1 targets
      `grimoire-rs/index` only. Its empty `trustedBots` is therefore not a gap
      to close here.
- [x] **Ordering — RESOLVED: sorting is in scope.** Not display-only. See
      **D14**: `Sort` widens to `"name" | "updated" | "rating"` across site,
      TUI, and a `grim search --sort` flag, with `"downloads"` reserved as an
      additive future value ([#89](https://github.com/grimoire-rs/grimoire/issues/89)).
      Order is rating desc → updated desc → name; unrated sorts last, never
      as zero.

## Links

- [`design_artifact_ratings.md`](../specs/design_artifact_ratings.md) — C4 model, full data flow, reconcile/tally algorithm, migration plan
- [`research_rating_backends.md`](../research/research_rating_backends.md) — ecosystem survey, forge API facts, performance
- [`research_rating_architecture_map.md`](../research/research_rating_architecture_map.md) — code map, seams, worktree ground truth
- [`research_rating_schema_compat.md`](../research/research_rating_schema_compat.md) — versioning, sidecar precedent, serde/TS mechanics
- [`research_rating_security.md`](../research/research_rating_security.md) — threat table, marker trust, bot identity
- [`research_rating_operability.md`](../research/research_rating_operability.md) — reconciler shape, triggers, publish, cost
- [`adr_projection_over_index.md`](./adr_projection_over_index.md) — the index is a phone book of pointers; reinforces the sidecar shape
- [`adr_git_provenance_annotations.md`](./adr_git_provenance_annotations.md) — closest template: opt-in feature, additive `CatalogEntry` fields, no version bump
- [`adr_catalog_summary_annotation.md`](./adr_catalog_summary_annotation.md) — optional field flowing end-to-end through the read path
- [`adr_catalog_freshness_revalidation.md`](./adr_catalog_freshness_revalidation.md) — **read its `Revision 2026-08-12` block, not the original proposal**
- [`adr_multi_registry_mcp.md`](./adr_multi_registry_mcp.md) — the `catalog_service::load_catalog` seam and per-registry cache keying
- [`adr_render_layout_stability.md`](./adr_render_layout_stability.md) — the additive/migration-promise pattern for a new on-disk artifact
- [`adr_announce_fork.md`](./adr_announce_fork.md) — the forge client, token ladder, and no-redirect hardening this reuses
- [Issue #82](https://github.com/grimoire-rs/grimoire/issues/82)

---

## Changelog

| Date | Author | Change |
|------|--------|--------|
| 2026-08-18 | architect (/hex-architect) | Initial draft — Proposed |
| 2026-08-18 | owner | **Status → Accepted.** All open markers resolved. |
| 2026-08-18 | owner | **`grim rate` confirms by default; `--yes` skips it.** A vote posts publicly under the user's own account, so it does not happen from a single un-flagged command. Scripting stays safe: a non-interactive caller without `--yes` gets an explicit `64` naming the flag rather than a hang or an unconfirmed vote, and `--token-stdin` requires `--yes` because stdin is already carrying the credential. Extension gate fixed at **`RATING_GRIM_VERSION = '0.14.0'`** (next minor after `v0.13.0`), closing the last open marker. |
| 2026-08-18 | plan Discover + codex (plan-artifact pass) | **Two factual corrections to an Accepted ADR**, both found while decomposing. (1) `findTrustedBot` is **login-keyed** and cannot serve R-1 clause 2's id lookup — that is a new id-keyed scan in the ratings code, and it must skip bare-string `trustedBots` entries whose `id` is `undefined` rather than wildcard-matching them. (2) The extension must add a **new `RATING_GRIM_VERSION`** gate, not bump `MINIMUM_GRIM_VERSION`, which is the hard floor for every existing command; `REGISTRY_EDIT_GRIM_VERSION` is the in-repo precedent. Also confirmed: the extension has **no** existing auth, `SecretStorage`, or `child.stdin` usage, so D13's credential flow is new infrastructure rather than reuse. Plan: [`plan_artifact_ratings.md`](../plans/plan_artifact_ratings.md). |
| 2026-08-18 | codex (cross-model adversary) | Independent pass on the post-acceptance shape. **4 Block findings, all valid, all fixed:** the rename left the design doc's tally algorithm and ER model emitting the old flat `entries[ref] = {up,target,url}`; R-2's carry-forward was still whole-file rather than per-stat-key; the seed's `curl … \|\| true` could not distinguish a genuine 404 from a transport failure, so a failed tally plus a failed seed would publish **no** sidecar — the exact wipe R-2 exists to prevent, through R-2's own mechanism; and **R-1 never bound a thread to its container**, so a Discussion transferred to another repo (or converted to an issue) kept its marker and author id and stayed authoritative — now **clause 4**, compared by immutable repo/project id. Warns fixed: `votes.json` now has a schema plus write/precedence/expiry rules, D13 specifies exact host comparison and every `--token-stdin` edge case, and rollback is an explicit two-part operation (disable producer **and** delete the published artifact — R-2 would otherwise serve a frozen rating set forever). Confirmed sound: the 72-vs-69 matrix arithmetic and the `all.json` byte-identity claim. |
| 2026-08-18 | owner | **Sidecar renamed `ratings.json` → `stats.json`** (named after the VS Code Marketplace `statistics` bag, which likewise holds ratings and installs together — recognition over novelty) and each ref now maps to a bag of stats (`entries[ref].rating`), with a per-stat `providers` block — done now because the published locator freezes at 1.0 and a later rename would mean serving a legacy path forever (**D14**). Rust/report field names deliberately unchanged. R-2 restated as a **per-stat-key** carry-forward. `lockThreads` config added (default true — no moderation burden, and it hardens R-1). All three `[NEEDS CLARIFICATION]` markers resolved: `index.grimoire.rs` ships ratings on at v1 (live enablement stays an owner action), `grimoire-lore` permanently out of scope, and **sorting is in scope** — `Sort` widens to name/updated/rating, unrated sorts last. |
| 2026-08-18 | owner | Source-repo stars **rejected permanently** rather than deferred: a skill teaching a tool lives in that tool's repo, so stars measure the product, and every artifact from one repo inherits an identical count — zero variance across the items being ranked. Download counts recorded as a separate, composing axis with a verified per-backend matrix; GHCR (the default) has no usable API. Tracked as [#89](https://github.com/grimoire-rs/grimoire/issues/89). |
| 2026-08-18 | review panel + owner | Panel findings folded in. **D12** (client-side vote state, tri-state, Invariant R-3) and **D13** (forge endpoint via host-matched credential lookup; `--token-stdin` host-matching) added. `botIds` duplicate registry removed — R-1 clause 2 now resolves author ids from `index-policy.json`'s `trustedBots[].id` (D5). Revisit trigger 4 (engagement never materialises) added; trigger 3 corrected — both forges *do* express "has this user voted". Coordinated Sybil inflation named alongside the self-upvote asymmetry. Reversibility claim qualified: a shipped `--format json` field can never be removed, only unpopulated. Docs plan retargeted `configuration.md` → `hosting-an-index.md` and gained the `subsystem-cli-commands.md` row plus the first-party catalog-drift duty. Status stays **Proposed**. |
