# Research: Operability & Cost of the Ratings Bot

<!--
Owner: hex-architect run 2026-08-17 (Research phase, operability/cost axis)
Handoff to: architect (ADR), /hex-plan
Extends: research_rating_backends.md — read that first. This file does not
restate the forge API facts, performance table, or design patterns already
settled there; it only adds the operability layer on top.
-->

## Metadata

**Date:** 2026-08-17
**Domain:** ci-cd
**Triggered by:** [#82](https://github.com/grimoire-rs/grimoire/issues/82) — Ratings for catalog artifacts
**Expires:** 2027-02-17 (re-verify GitHub/GitLab pricing and rate-limit numbers — both moved in the last 12 months)

## Direct Answer

The bot job is a **level-triggered reconciler with no external state** — same
shape as `kube-controller-manager`, `release-please`, and `stale`: list what
exists on the forge, diff against desired, act on the delta, and let the next
run re-derive everything from scratch. That property is *already* the
checkpoint, so thread creation needs no persisted cursor, only a **per-run
creation budget** paced under GitHub's 80/min · 500/hr secondary limit.
`ratings.json` should **never be committed** — publish it straight into the
Pages artifact (`actions/upload-pages-artifact` / GitLab Pages job
`artifacts:`) so a vote never produces a git commit, a CI-loop risk, or repo
bloat. The two hazards worth escalating to the architect: (1) **concurrent
reconciler runs race on thread creation** — nothing in either forge API makes
"create if not exists" atomic, so `concurrency:` / `resource_group` is a
correctness requirement, not housekeeping; (2) GitHub **disables scheduled
workflows after 60 days of no *commit* activity** on public repos, and
running the workflow does not count as activity — the schedule is not a
reliable safety net for exactly the quiet indexes that need one most.

## 1. Reconciler shape — prior art survey

None of the five bots surveyed keep a database of "which objects I own."
Each rebuilds that answer by querying the platform every run, using one of
two anchors: a **name/branch convention** or an **embedded marker**.

| Bot | How it finds its own objects | Duplicate-avoidance | State kept outside the platform |
|---|---|---|---|
| [Renovate](https://docs.renovatebot.com/getting-started/installing-onboarding/) | Fixed branch name (`renovate/configure` for onboarding, `branchPrefix` + slug per update) | Branch existence check; `branchPrefixOld` covers a rename so old branches still match | None — branch name *is* the state |
| [Dependabot](https://nesbitt.io/2026/01/02/how-dependabot-actually-works.html) | GitHub's proprietary infra passes an `existing-pull-requests` list into each job; dependabot-core itself is stateless — "clones your repo, parses manifests, checks registries, outputs file changes, and exits" | Server-side list diffed against desired updates before dependabot-core runs | None on the OSS side; [dependabot-gitlab](https://nesbitt.io/2026/01/02/how-dependabot-actually-works.html)'s self-hosted reimplementation had to add Postgres + `FOR UPDATE SKIP LOCKED` row locking to replace what GitHub's infra does for free |
| [release-please](https://loiccoyle.com/posts/release_please/) | Searches for an open PR carrying the `autorelease: pending` label | Label presence check before opening a new PR; found PR is *updated* in place, not recreated | None — label *is* the state machine (`pending` → `tagged` on merge) |
| [`actions/stale`](https://github.com/actions/stale) | Re-scans all open issues/PRs every run, filtered by `updated_at` age | Idempotent by construction — reapplying the stale label/comment to an already-stale issue is a no-op; a bounded `operations-per-run` budget resumes "from the first unprocessed issue" next run using the Actions cache | Actions cache holds only a resume cursor, not ownership state |
| [all-contributors](https://allcontributors.org/en/bot/overview/) | `<!-- ALL-CONTRIBUTORS-LIST:START -->...END` HTML comment markers in the README | Parses the marked block, adds/updates rows, never duplicates because the block is the single source of truth | None — the marker *is* the state |

Two properties repeat across all five, and both match what
[`research_rating_backends.md`](./research_rating_backends.md) already
adopted (embedded `<!-- grim-ref: ... -->`, no back-push into the publish
path):

1. **No bot keeps a second copy of "what I created."** The forge object
   (branch name, label, marker text) *is* the durable record. A lost
   database, a wiped Actions cache, or a fresh clone all recover to the same
   state on the next run — this is exactly Kubernetes' level-triggered
   argument: ["given the current object graph, converge toward desired
   state"](https://hackernoon.com/level-triggering-and-reconciliation-in-kubernetes-1f17fe30333d),
   resilient to "missed events, external changes, partial failures during
   reconciliation."
2. **The only state any of them persist is a *resume cursor* for a
   rate-limited or budget-capped operation** (`stale`'s "first unprocessed
   issue," Renovate's branch cache) — never ownership truth. `kubebuilder`'s
   default `SyncPeriod` (10h, ±10% jitter) exists for the same reason: a
   periodic **full resync** catches drift an event-driven pass missed,
   without requiring the controller to remember what it last saw.

**Recommendation:** the tally/reconcile job needs **zero external state**.
Each run:

1. List every existing forge thread carrying `<!-- grim-ref: -->` (scoped to
   the announcement category / a bot label, per the backend research), build
   the observed-ref set.
2. Diff against the catalog's desired-ref set (from the just-built
   `all.json`).
3. Create threads for missing refs, capped by the per-run budget (§2).
4. Tally `upvoteCount`/`awardEmoji` across all observed threads, write
   `ratings.json` if changed.

Because step 1 recomputes the observed set from the forge every run, a
partial run (rate-limited, cancelled, crashed) leaves *fewer threads
created* — not corrupt state. The next run's list-and-diff simply finds more
missing refs and continues. **This is the checkpoint** — no cursor file, no
database, no committed progress marker needed for correctness. A resume
cursor is a worthwhile *optimization* once caught up (skip re-listing 10k
threads to find the same zero missing refs every hour), but must never be
the thing correctness depends on, mirroring `kubebuilder`'s resync-on-top-of-
events design.

## 2. Throttled backfill

GitHub's own [best-practices
doc](https://docs.github.com/en/rest/using-the-rest-api/best-practices-for-using-the-rest-api?apiVersion=2026-03-10)
is explicit and directly actionable:

- **"Make requests serially instead of concurrently"** to avoid the
  secondary limit — concurrency is the documented failure mode, not an
  edge case.
- **Retry order**: honor `retry-after` first; if `x-ratelimit-remaining: 0`,
  wait until `x-ratelimit-reset` (epoch seconds); otherwise back off
  exponentially starting at ≥60s.
- **"Continuing to make requests while you are rate limited may result in
  the banning of your integration"** — a naive retry loop that ignores
  `Retry-After` is a real risk to the bot's credential, not just wasted
  quota.
- Conditional requests (`If-None-Match` / `If-Modified-Since`) return `304`
  and **do not count against the primary limit** — worth using for the tally
  list pass once the artifact set stabilizes, though at ~100 requests/10k
  artifacts this is optimization headroom, not a requirement.

**Client library**: [`@octokit/plugin-throttling`](https://github.com/octokit/plugin-throttling.js/)
is the reference implementation of exactly this contract — built on
Bottleneck, it exposes `onRateLimit`/`onSecondaryRateLimit` callbacks that
receive `retryAfter` and decide whether to retry, implementing "all
recommended best practices to prevent hitting secondary rate limits." Do not
hand-roll the backoff loop; use this plugin (or the GraphQL-side equivalent —
query `rateLimit { remaining, resetAt }` before each batch, which is the
GraphQL analog of checking `x-ratelimit-remaining`) rather than re-deriving
these rules.

**Recommended backfill shape** for thread *creation* (the only rate-limited
path — tallying and listing are cheap per §1 and the backend research's
performance table):

- Fixed conservative per-run budget, e.g. **400 creations/run**, leaving
  headroom under the 500/hour secondary cap for the concurrent tally/list
  calls in the same run.
- Process missing refs in a **stable deterministic order** (sorted by ref
  string) so successive runs make monotonic progress with no stored offset —
  consistent with the "no external state" recommendation in §1.
- On a secondary-limit response, back off per the throttling plugin's
  default and stop the run early rather than spinning — next scheduled run
  picks up the remainder for free via the same list-and-diff.
- Log one line: `created X/Y threads this run, Z remaining` — this single
  number is what makes "backfill still draining" observable (§6) without any
  dashboard.

At 10k artifacts this yields a ~25-run backfill (10,000 ÷ 400) rather than
the worst-case 20-hour *single* cold start the backend research flagged —
spread automatically across scheduled runs with zero coordination code,
because the reconciler is idempotent and stateless by construction.

## 3. Scheduled jobs on both CI systems

| Concern | GitHub Actions `schedule` | GitLab CI scheduled pipelines |
|---|---|---|
| Minimum interval | 5 minutes (finer cron is silently coalesced/dropped) — [cronbuilder.dev](https://cronbuilder.dev/blog/github-actions-cron-schedule.html) | 1 hour minimum on **GitLab.com free tier**; **no cap on self-managed/paid** — [pipeline efficiency docs](https://docs.gitlab.com/ci/pipelines/pipeline_efficiency/) |
| Timing reliability | **Not guaranteed** — GitHub documents best-effort delivery; real-world reports show 5–30 min delays at peak (around `:00`/`:30`), 30–60 min under load, and the queue can **silently drop a run entirely** with no trace — [dev.to writeup](https://dev.to/krissv/monitoring-github-actions-scheduled-workflows-a-practical-guide-31h7), [substack analysis](https://lowlysre.substack.com/p/predicting-github-cron-delays?open=false) | Requires the schedule owner to hold Developer role and (on protected branches) merge rights, and a valid `.gitlab-ci.yml` — no documented equivalent skew, but self-managed runner availability is on you |
| Auto-disable | **Public repos**: scheduled workflows disable after **60 days with no commit/push/release/merge activity** — issue comments and stars don't count, and *running the scheduled workflow itself does not reset the clock* — [GitHub Docs](https://docs.github.com/actions/managing-workflow-runs/disabling-and-enabling-a-workflow) | No equivalent documented |
| Concurrency control | `concurrency: { group, cancel-in-progress }` at workflow level | `resource_group` with `process_mode: oldest_first` queues rather than races |
| Overlap default | Same-group runs **cancel** the pending one by default; must explicitly set `cancel-in-progress: false` to queue instead | `resource_group` explicitly queues; without one, GitLab runs overlapping scheduled pipelines concurrently |

**Recommendation:**

- Treat the **push-triggered run (every index build)** as primary — it fires
  on real commits, which is exactly the activity that resets GitHub's 60-day
  clock, so it never silently dies.
- Keep the **schedule as a secondary safety net** for drift between builds
  (new votes arriving with no new artifact published), but do not rely on it
  alone — document that on a quiet GitHub-hosted index, the schedule can
  silently stop firing after 60 days of no other repo activity while the
  push-triggered path keeps working fine as long as *someone* publishes.
- **Must** set `concurrency: { group: grim-ratings-bot, cancel-in-progress: false }`
  (Actions) or a `resource_group` (GitLab) on the reconcile job. This is not
  about wasted CI minutes — see the race hazard in §6.
- Pick an hourly-or-coarser schedule cron regardless of platform: it clears
  GitLab.com's free-tier 1-hour floor, stays well inside GitHub's 5-minute
  floor with margin for the documented delay jitter, and matches the
  reconciler's actual freshness need (votes are not latency-sensitive).

## 4. Committing vs. publishing derived artifacts

The settled design's "write only when the tally changed" rule reduces commit
*frequency* but does not remove the structural problems of committing at
all, and it is **not sufficient on its own**:

- **CI-loop risk remains, just throttled, not eliminated.** If
  `ratings.json` lives in the repo and the reconciler job is triggered "after
  every index build" (i.e., on push), then any run that *does* produce a
  changed tally commits, which — if that push is itself a build trigger —
  fires a second build. This doesn't runaway (the second run's tally is
  already current, so it makes no further commit), but it doubles the
  build/deploy latency and CI-minute cost for every vote that lands between
  scheduled runs. The conventional guard is a **`[skip ci]` marker in the
  bot's commit message**, which GitHub Actions has recognized natively since
  [Feb 2021](https://github.blog/changelog/2021-02-08-github-actions-skip-pull-request-and-push-workflows-with-skip-ci/)
  — but this is a patch on a problem that publishing avoids by construction.
- **History bloat compounds over years.** A file that changes on every vote,
  committed to the default branch, accumulates a diff-noise commit history
  indefinitely — exactly the case the [gh-pages branch
  convention](https://github.com/tschaub/gh-pages) was invented to avoid by
  separating "files used to generate the site" from "the generated output,"
  and even that convention still writes a commit per deploy.
- **The clean fix is to never commit it.** Both target platforms support
  **artifact-based Pages deployment with no git commit at all**:
  - GitHub: `actions/upload-pages-artifact` packages the build output
    (including a freshly-written `ratings.json`) and `actions/deploy-pages`
    publishes it directly — [no branch, no commit, artifact retained 1 day
    pending deploy](https://github.com/actions/deploy-pages).
  - GitLab: a `pages` job's `artifacts: paths:` (default `public/`, or any
    directory via `pages.publish` since 17.10) is picked up by the Pages
    deploy stage directly from job artifacts — [GitLab Docs](https://docs.gitlab.com/ci/jobs/job_artifacts/).

  This removes the loop risk entirely (a Pages deploy is not a `push`
  event), removes history bloat, and removes the need for `[skip ci]`
  bookkeeping. **Recommendation: drop the "write only when changed" commit
  path in favor of always regenerating `ratings.json` into the Pages
  artifact on every run** — regeneration is cheap (§1, §7) and "only when
  changed" stops being a cost concern once it's not a commit.

## 5. Generated CI as a distribution mechanism

The indexer currently generates CI config *into* each downstream index repo
— a vendored-copy model. The comparable-tooling landscape draws a sharp line
between two upgrade strategies:

- **Vendored generation (cookiecutter-style, no update path).** Cookiecutter
  scaffolds once; there is no built-in mechanism to propagate a later
  template change into already-generated repos — the well-known gap that
  [`cruft`](https://cruft.github.io/cruft/) exists to patch after the fact
  (`cruft diff` / `cruft update`), bolted on rather than designed in.
- **Tracked generation with 3-way merge (Copier-style).** [Copier](https://dev.to/cloudnative_eng/copier-vs-cookiecutter-1jno)
  records the template version and the answers used at generation time; a
  later `copier update` performs a 3-way merge between the original
  template, the new template version, and the user's actual file —
  "respects manual changes made by developers" rather than blindly
  overwriting.
- **Thin generated stub that references a versioned upstream (reusable
  workflow / remote include).** This sidesteps the drift problem entirely by
  not vendoring the logic at all:
  - GitHub reusable workflows: `uses: org/repo/.github/workflows/x.yml@v1.2.3`
    — a tag, SHA, or branch ref; Dependabot can bump the pin like any other
    dependency ([changelog](https://github.blog/changelog/2023-03-13-dependabot-updates-support-reusable-workflows-for-github-actions/)).
  - GitLab: `include: - project: 'grimoire-rs/ci-templates' ref: v2.0.0 file: '/ratings.yml'`,
    or the newer [CI/CD Components
    Catalog](https://docs.gitlab.com/ci/components/) — versioned,
    parameterized (`inputs:`), designed exactly for this "one org, many
    consuming repos" shape.

**Recommendation: generate a thin stub, not the job body.** The indexer
should emit a few lines per downstream repo — a job that does `uses:
grimoire-rs/index-ci-workflows/.github/workflows/ratings.yml@v1` (GitHub) or
`include: { component: 'grimoire-rs/ci-components/ratings@1' }` (GitLab) —
rather than inlining the reconciler's full logic into every generated
`.gitlab-ci.yml`/`workflow.yml`. This is the mechanism that **minimizes
upgrade burden**: a fix or rate-limit-budget tweak ships once, in the
upstream workflow/component, and every downstream repo picks it up on its
next run without regenerating anything. It also sidesteps the drift-
detection problem outright — there is no vendored copy in the downstream
repo to drift from the source of truth, so no `cruft diff`-equivalent
tooling is needed for *this* file (the surrounding generated CI scaffolding
that genuinely must be vendored — checkout, install, secrets wiring — is a
separate, smaller surface where Copier-style versioned regeneration remains
the right fallback if `npx @grimoire-rs/indexer init` needs a re-run path
later). Pin by tag, let Dependabot/Renovate bump it like any other action
reference, and document the version pin in `index.config.json` so
`grim`/the indexer can report which ratings-workflow version a given index
is running.

## 6. Observability and failure modes for an unattended bot

Nobody watches an index repo's Actions tab. The minimum useful signal set,
derived from what the reconciler naturally knows each run:

- **One structured log line per run**, not a dashboard:
  `refs=N created=X/Y tallied=Z changed=<bool> secondary_limit_hit=<bool>`.
  This single line answers "is the backfill still draining" (`X < Y`),
  "did we get throttled" (`secondary_limit_hit`), and "is anything even
  happening" (`changed`) without external infrastructure.
- **Fail loudly, not silently, on anything that isn't the documented
  degraded path.** An HTTP error other than a rate limit (auth failure,
  category/work-item-type missing, GraphQL schema error) should **exit
  non-zero** — a red X in the Actions tab is the only alert an unattended
  bot gets. GitHub's own notification default only reaches "the person who
  triggered the workflow," which for a `schedule` trigger is effectively
  nobody — so failing the job (rather than logging-and-continuing) is what
  makes the red X exist at all; email/Slack routing on top is optional
  polish, not the baseline. `dawidd6/action-send-mail` or a
  `workflow_call`-shared notification job are the standard lightweight
  add-ons if the team wants push notification, per the
  [alerting research](https://ravgeetdhillon.medium.com/send-an-email-notification-when-github-actions-fails-ea83cbeabbe0) — reasonable to defer to a follow-up issue rather than build day one.
- **"Tally unchanged for N runs" is a derived signal, not a stored one** —
  since the log line already reports `changed=<bool>` every run, a maintainer
  (or a cheap follow-up: a tiny badge/counter script) can compute "last
  changed N runs ago" from the Actions run history without the bot
  persisting anything extra. Don't build a metrics pipeline for this.
- **Rate-limit hits should warn, not fail**, the run that got throttled
  mid-backfill did real, correct, partial work (§1's self-healing property)
  — treat `secondary_limit_hit=true` as a scheduled-continuation signal in
  the log, not a red X.

## 7. Cost

| | At ~200 artifacts | At 10,000 artifacts |
|---|---|---|
| CI minutes/run (reconcile+tally) | Seconds of API calls + index build time already paid for — negligible marginal minutes | List/tally ~100 sequential requests (§1, backend research) — under a minute of API time; creation-budgeted backfill run stays within the same job timeout since it's capped at ~400 creates/run |
| API quota | 2 GraphQL requests for tally; well under the 5,000 pts/hr primary and 2,000 pts/min limits | ~100 GraphQL requests for tally (~100 pts of 5,000/hr); creation path is the binding constraint at 80/min · 500/hr, not point budget |
| `ratings.json` size/bandwidth | Negligible | ~300KB gzipped full, ~75KB with the zero-vote-omission already adopted in the backend research — trivial Pages bandwidth at any realistic index traffic |
| Human maintenance | Effectively zero once the workflow/component is generated — no service to patch, no DB to back up | Same, plus one-time attention while the ~25-run backfill (§2) drains after crossing into five figures |
| **Hosted-runner $ cost (as of Aug 2026)** | GitHub-hosted Linux runner: **$0.006/min** (post the [Jan 2026, up-to-39% cut](https://cicdcost.com/github-actions-pricing)); public repos free regardless. A sub-minute reconcile job costs a fraction of a cent | Same rate; even a full backfill run stays well under a minute of actual compute — the constraint is the rate limit, not runner time |
| **Self-hosted runners** | **Free** on both platforms as of today — GitHub's proposed $0.002/min self-hosted platform fee (announced Dec 16, 2025, to start Mar 1, 2026) was **postponed within 48 hours** after community backlash and has not been reinstated as of Aug 2026 ([Techzine](https://www.techzine.eu/news/devops/137396/github-bends-to-criticism-and-delays-paid-self-hosting-of-runners/), [socket.dev](https://socket.dev/blog/github-actions-pricing-whiplash)); GitLab self-managed runners have **no per-minute charge on any tier**, "unlimited CI minutes" | Same — cost is whatever compute the org already runs, not the ratings job specifically |
| **Corporate self-hosted (GHES / self-managed GitLab)** | No documented platform-fee change applies to GHES; self-managed GitLab has no CI-minute billing at all. The bot's cost there is indistinguishable from any other lightweight scheduled job on infrastructure the org already operates | Same — the 10k-artifact ceiling is a *rate-limit* problem (backfill duration), not a cost problem, at self-hosted scale |

**Bottom line:** this job is free-tier-shaped at every scale considered. The
real cost driver the settled design already identified — the 20-hour cold
backfill at 10k artifacts — is a **rate-limit wall clock problem**, not a
CI-minutes or dollar problem; §2's budgeted-per-run approach converts it into
~25 unattended scheduled runs instead of a monitored 20-hour foreground job,
which is the operationally cheap way to spend that wall-clock time.

## 8. Local testing without publishing

**`file:` dependency reliability.** npm's own behavior confirms this is
solid for the two-worktree loop the owner wants: installing a `file:`
local-path dependency **creates a symlink** into `node_modules` (same
mechanism as workspaces and `npm link`) rather than copying —
[npm CLI issue #4031](https://github.com/npm/cli/issues/4031) discusses
this as the standing behavior, and [npm's workspaces
docs](https://docs.npmjs.com/cli/link) describe the same symlink mechanism.
Practical implications for the dev loop:

- Edits to the indexer worktree are visible to the index worktree
  **immediately**, no reinstall — because it's a symlink, not a copy. This
  is exactly what "hard-resettable between runs" wants: reset state by
  deleting generated output in the index worktree, not by reinstalling the
  dependency.
- The one documented gotcha: `install-links` (which packs `file:`
  dependencies as regular non-symlinked deps) **has no effect inside
  workspaces** — irrelevant here since this is a plain two-worktree `file:`
  reference, not an npm workspace, but worth stating so nobody "fixes" a
  symlink they didn't expect by reaching for that flag.
- Caching pitfalls exist in build-cache tooling that hashes `file:` deps by
  version string rather than content (e.g. [Moon's cache-invalidation gap on
  `file:` deps](https://github.com/moonrepo/moon/issues/2055)) — irrelevant
  to plain `npm install`/`npm run`, which always re-reads through the
  symlink; only matters if a build-cache layer gets added later.
- **`npm link` vs `file:`**: functionally near-identical (both symlink), but
  `file:` is the better choice here because it's declared in
  `package.json`'s `devDependencies` and survives a fresh `npm ci` in the
  same worktree pair without a separate `npm link` step per clone — matches
  "two worktrees, hard-resettable" better than a global-link workflow that
  has its own [documented
  gotchas](https://dev.to/privatenumber/4-reasons-to-avoid-using-npm-link-5d03).
- **npm workspaces** were considered and rejected for this specific loop:
  workspaces assume both packages live under one repo root, which conflicts
  with "two worktrees" as separate checkouts (the indexer and an index
  instance are different repos with independent histories) — `file:` across
  worktrees is the right-shaped tool, workspaces would force a monorepo
  structure neither package wants.

**Faking the forge API.** Three real options, in order of cost:

| Approach | Cost | Fidelity | Verdict for this job |
|---|---|---|---|
| Recorded fixtures / VCR cassettes | Cheapest to add, but must be re-recorded whenever the GraphQL query shape changes | High for the exact recorded shape, blind to anything not recorded | Good for regression tests of the tally-parsing logic once the schema is stable; brittle as the primary dev-loop tool while the mutations are still being iterated on |
| Local mock server (nock / MSW) | Moderate — write handlers for the specific GraphQL operations used (`discussions`, `addUpvote`, `awardEmojiToggle`) | High, and easy to simulate the exact failure modes §2/§6 need to exercise (secondary-limit response, partial page, malformed marker) | **Recommended for unit/integration tests** — [Probot's own test suite uses nock](https://probot.github.io/docs/testing/) for exactly this shape (bot logic against a mocked GitHub API), and MSW's GraphQL handlers cover the GitLab work-items path the same way |
| Provider interface + in-memory implementation | Moderate, but this is architecture the bot needs anyway — the backend research already treats `provider.kind` as the swappable seam | Full fidelity for reconcile-loop *logic* (diffing, budget pacing, marker parsing) with zero network code in the fast path | **Recommended for the local dev loop itself** — an in-memory `RatingsProvider` (a map of ref → thread + votes) lets `npx @grimoire-rs/indexer` run the whole reconcile+tally pass against a fake forge with no HTTP, no fixtures to maintain, and a one-line reset between runs |

**Recommended setup, cheapest-that-actually-exercises-the-logic:** (1) two
worktrees + `file:` `devDependencies` pointer, reset by `git clean -fdx` in
the index worktree between runs (cheap because nothing there is a real
publish target); (2) an **in-memory provider implementation** behind the
same interface the real GitHub/GitLab providers use, seeded from
`all.json`, exercised by the indexer's own test suite to prove
reconcile+tally correctness (duplicate-avoidance, budget pacing, marker
round-trip) without any network; (3) **nock/MSW-based fixtures** layered on
top only for the thin adapter code that actually serializes GraphQL
requests/responses — the part an in-memory fake can't validate, and the only
part that should ever need re-recording when GitHub/GitLab change a
response shape.

## Key Findings

1. Every comparable bot (Renovate, Dependabot, release-please, `stale`,
   all-contributors) keeps **no external ownership state** — the forge
   object itself (branch, label, marker) is the durable record, which is
   exactly the `<!-- grim-ref: -->` design already adopted. This makes the
   reconciler naturally resumable with no checkpoint file required.
2. GitHub's secondary content-creation limit (80/min · 500/hr) is the only
   real throughput constraint; tallying and listing stay cheap at any
   catalog size the backend research projected. A fixed per-run creation
   budget (e.g. 400/run) turns the flagged 20-hour cold backfill into ~25
   unattended scheduled runs with zero coordination code.
3. **Concurrent reconciler runs are not safe** — neither forge API offers an
   atomic "create thread if none exists for this ref," so two overlapping
   runs (a push trigger racing a schedule trigger) can create duplicate
   threads for the same artifact. `concurrency:`/`resource_group` is a
   correctness requirement, not a cost optimization.
4. GitHub disables scheduled workflows on public repos after 60 days with no
   **commit** activity, and running the scheduled workflow itself doesn't
   reset that clock — the schedule is not a trustworthy safety net for
   exactly the low-traffic indexes most likely to need one; the
   push-triggered-by-index-build path is the one to rely on.
5. Committing `ratings.json` is unnecessary and costly (loop risk, history
   bloat) when both platforms support artifact-based Pages deploys with
   **zero git commits** — `actions/upload-pages-artifact` +
   `actions/deploy-pages`, or a GitLab `pages` job's `artifacts: paths:`.
   "Write only when changed" is a good rule for a committed file and
   irrelevant once nothing is committed.
6. Generated CI should be a **thin stub referencing a versioned upstream
   workflow/component** (`uses: …@v1` / `include: component:`), not vendored
   job bodies — this is the only one of the surveyed distribution models
   (vendored generation, tracked 3-way-merge regeneration, thin
   stub-and-reference) that requires zero re-generation of downstream repos
   to ship a fix.
7. As of August 2026 this job is free-tier-shaped at any realistic scale on
   both platforms — GitHub's proposed self-hosted-runner platform fee was
   announced and then postponed within 48 days of community backlash and
   remains inactive; the 10k-artifact "problem" is a rate-limit wall-clock
   issue, not a dollar-cost issue.
8. `file:` npm dependencies across two worktrees are symlinks, not copies —
   this is the cheap, reliable local dev loop the owner wants, with no
   caching gotcha relevant outside a build-cache layer neither worktree has.

## Recommendation

Ship the reconciler as a **stateless, level-triggered loop** (list → diff →
budgeted-create → tally → publish-not-commit), driven primarily by the
push-triggered index build with the schedule as a secondary resync, gated by
a `concurrency`/`resource_group` lock for correctness. Distribute it as a
versioned upstream workflow/component referenced by a thin generated stub,
not vendored logic. Test it locally against an in-memory provider fake
behind two `file:`-linked worktrees, with nock/MSW reserved for the thin
serialization adapter. None of this requires new infrastructure, a
database, or ongoing human attention beyond watching for a red CI X — which
is also the one thing worth making sure actually happens (fail loud on
anything that isn't a documented rate-limit backoff).

## Sources

| Source | Type | Relevance |
|---|---|---|
| [Renovate onboarding docs](https://docs.renovatebot.com/getting-started/installing-onboarding/) | Docs | Branch-name-as-state pattern |
| [How Dependabot Actually Works](https://nesbitt.io/2026/01/02/how-dependabot-actually-works.html) | Blog | Stateless job model, `existing-pull-requests` list, dependabot-gitlab's Postgres reimplementation |
| [release-please](https://loiccoyle.com/posts/release_please/) | Blog | Label-as-state-machine pattern |
| [`actions/stale`](https://github.com/actions/stale) | Repo | Resume-cursor-not-ownership-state pattern |
| [all-contributors bot overview](https://allcontributors.org/en/bot/overview/) | Docs | Marker-block-as-state pattern |
| [Level Triggering and Reconciliation in Kubernetes](https://hackernoon.com/level-triggering-and-reconciliation-in-kubernetes-1f17fe30333d) | Blog | Level- vs edge-triggered rationale |
| [Kubebuilder reconciliation loop](https://deepwiki.com/kubernetes-sigs/kubebuilder/5.2-reconciliation-loop) | Docs | Default 10h SyncPeriod / resync-on-top-of-events |
| [GitHub REST API best practices](https://docs.github.com/en/rest/using-the-rest-api/best-practices-for-using-the-rest-api?apiVersion=2026-03-10) | Docs | Serial requests, `Retry-After`, conditional requests, backoff order |
| [`@octokit/plugin-throttling`](https://github.com/octokit/plugin-throttling.js/) | Repo | Reference throttling implementation |
| [GitHub Actions cron minimum interval](https://cronbuilder.dev/blog/github-actions-cron-schedule.html) | Blog | 5-minute floor |
| [Scheduled workflow delay reporting](https://dev.to/krissv/monitoring-github-actions-scheduled-workflows-a-practical-guide-31h7) | Blog | 5–30min+ documented skew |
| [Predicting GitHub cron delays](https://lowlysre.substack.com/p/predicting-github-cron-delays?open=false) | Blog | Root cause of dispatch delay |
| [Disabling/enabling a workflow](https://docs.github.com/actions/managing-workflow-runs/disabling-and-enabling-a-workflow) | Docs | 60-day inactivity auto-disable, commit-only activity |
| [GitLab pipeline efficiency](https://docs.gitlab.com/ci/pipelines/pipeline_efficiency/) | Docs | 1-hour SaaS-free-tier scheduled-pipeline floor |
| [GitLab resource groups](https://docs.gitlab.com/ci/resource_groups/) | Docs | `resource_group` / `process_mode: oldest_first` |
| [GitHub Actions concurrency](https://docs.github.com/en/actions/writing-workflows/choosing-what-your-workflow-does/control-the-concurrency-of-workflows-and-jobs) | Docs | `cancel-in-progress` default behavior |
| [`[skip ci]` changelog](https://github.blog/changelog/2021-02-08-github-actions-skip-pull-request-and-push-workflows-with-skip-ci/) | Changelog | Native skip-ci support |
| [`actions/deploy-pages`](https://github.com/actions/deploy-pages) | Repo | Artifact-based Pages deploy, no branch commit |
| [GitLab job artifacts](https://docs.gitlab.com/ci/jobs/job_artifacts/) | Docs | Pages `artifacts: paths:` deploy, no commit |
| [Cruft](https://cruft.github.io/cruft/) | Docs | Cookiecutter drift-detection bolt-on |
| [Copier vs Cookiecutter](https://dev.to/cloudnative_eng/copier-vs-cookiecutter-1jno) | Blog | 3-way-merge update model |
| [Dependabot reusable-workflow support](https://github.blog/changelog/2023-03-13-dependabot-updates-support-reusable-workflows-for-github-actions/) | Changelog | Version-pinned `uses:` upgrade path |
| [GitLab CI/CD Components](https://docs.gitlab.com/ci/components/) | Docs | Versioned, parameterized shared pipeline units |
| [Probot testing docs](https://probot.github.io/docs/testing/) | Docs | nock-based bot-vs-mocked-GitHub-API testing pattern |
| [npm CLI issue #4031](https://github.com/npm/cli/issues/4031) | Issue | `file:` local-path symlink behavior |
| [Moon `file:` cache-invalidation gap](https://github.com/moonrepo/moon/issues/2055) | Issue | Build-cache-layer-only caveat, not plain npm |
| [GitHub Actions 2026 pricing](https://cicdcost.com/github-actions-pricing) | Blog | Post-cut $0.006/min Linux hosted-runner rate |
| [GitHub self-hosted runner fee postponement](https://www.techzine.eu/news/devops/137396/github-bends-to-criticism-and-delays-paid-self-hosting-of-runners/) | News | Dec 2025 announcement, 48h reversal, status as of Aug 2026 |
| [GitHub Actions pricing whiplash](https://socket.dev/blog/github-actions-pricing-whiplash) | Blog | Timeline and exemptions (public repos, GHES) of the postponed fee |
