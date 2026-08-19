# Artifact Ratings

An index lists what exists. It says nothing about what is any good — and a
catalog of two hundred skills with no signal at all is a catalog nobody
browses twice.

Every ecosystem that solved this built a service to hold the votes: npm,
[Open VSX][open-vsx], the VS Code marketplace. Grimoire cannot, and would
not want to. *Storage is any OCI registry — GHCR, Docker Hub, or your own.
There is no Grimoire service to sign up for*, and adding one just to count
upvotes would trade the whole architecture for a star rating.

So ratings reuse a database every index already has: **the forge the index
repository lives on**. One [GitHub Discussion][gh-discussions] (or one
[GitLab work item][gl-work-items]) per package, and the vote is the forge's
own upvote or [emoji reaction][gl-reactions] — cast by the user, under the
user's own account, visible on the forge, revocable there. A scheduled job
in the index's own pipeline tallies the counters into a `stats.json` file
served beside `all.json`. Nothing new is hosted, nothing new is
authenticated, and no vote record exists anywhere Grimoire operates.

This page is for the person who runs an index. If you only want to *cast* a
vote, [`grim rate`][commands-rate] is the whole story.

## How a Vote Travels {#workflow}

Two paths, deliberately unconnected. Reading a rating is static file
distribution; writing one is a live call to a forge. They meet only at the
`stats.json` file, and only in one direction.

### Reading: a file beside `all.json` {#workflow-read}

An index that has ratings on publishes `<base>/stats.json` next to its
[`all.json`][spec-compiled]. grim fetches it in the same pass as `all.json`
and joins it onto the catalog by artifact ref, so a rating costs one extra
`GET` per catalog rebuild and nothing per package. The result lands in the
ordinary catalog cache under the ordinary one-hour TTL — there is no
separate rating cache, no rating request at browse time, and no rating
traffic at all under [`--offline`][offline].

Ratings ride the **HTTP index transport only**. A [git-transport
index][self-hosting-git] is a tree of `metadata.json` files with no sidecar
to fetch, and an OCI registry browsed through `_catalog` has no index
document at all; both read unrated, permanently and without a warning.

**Absent is never an error.** No `stats.json` (the normal case — most
indexes run no tally), no entry for a ref, a `rating` key missing from an
entry that carries other statistics, a `schema_version` from a future
release, a truncated body, a TLS failure: every one of them means *unrated*
for the refs it covers, logged at `debug`, and the catalog build still
succeeds. Nothing about ratings can fail a browse.

Where the counts show up:

| Surface | Rating |
|---|---|
| `grim search --format json` | A `rating` object `{up, url}`, or `null` — see [the JSON interface][json-search] |
| `grim search --sort rating` | Orders the browse by upvotes; see [`--sort`][commands-search-sort] |
| [`grim tui`][commands-tui] | A `Rating:` row in the detail pane, and `--sort rating` |
| The index's own catalog site | A vote count per card, and a rating sort tab |
| The [VS Code extension][vscode] | The count, plus a vote affordance |

### Writing: a call to the forge {#workflow-write}

[`grim rate <ref>`][commands-rate] resolves the row in the catalog, reads
the opaque vote target the index published for it, and issues the forge's
own mutation — `addUpvote`/`removeUpvote` on GitHub, `awardEmojiToggle` on
GitLab. It authenticates as the user, with the user's own credential, and
posts nothing but the vote.

Because the vote is public and carries the user's name, an interactive run
**confirms first**:

```console
$ grim rate ghcr.io/acme/skills/code-review
This posts publicly to your github account as alice. Continue? [y/N] y
```

grim writes no vote record of its own beyond a small local cache
(`$GRIM_HOME/state/votes.json`) remembering whether *this* account has
already voted, so the UI can render the right affordance without asking the
forge on every frame. It is discardable at any time; deleting it costs one
round trip.

### What the three invariants buy you {#workflow-guarantees}

The design has three properties an operator should be able to state to
their own users. Each is enforced by tests, not by convention.

**A forged marker cannot inflate a count.** The tally learns which thread
belongs to which artifact from an HTML comment — `<!-- grim-ref: … -->` —
in the thread body. Anyone can type that comment. It binds a ref only when
*all four* of these hold: it is in the body of a **top-level thread**, never
a comment or reply; the thread's author id is listed in
`index-policy.json`'s `trustedBots[].id`; it is the **first** anchored match
in that body; and the thread still lives in the configured
repository/project **and** category/work-item type, compared by immutable
numeric id rather than by name. A ref bound by two authorized threads is a
**conflict** and contributes **zero** votes, with both URLs logged, rather
than a number nobody can audit. Threads are locked on creation by default,
so the reply this rule rejects usually cannot be posted in the first place.

**A failed tally never empties a published count.** The deploy carries
forward, per statistic key, anything the run did not successfully produce.
A tally that could not reach the forge, was throttled halfway through a
listing pass, or never ran at all leaves the previously published numbers
exactly where they were — stale, never wiped. The seed fetch **branches on
its status code** instead of swallowing failures: a `404` is an empty seed,
a `2xx` that parses is the merge base, and an unparseable body, a 5xx, a
TLS error or a timeout **fails the job** rather than silently deploying a
site with no ratings on it.

**The UI never claims you have not voted when it does not know.** Vote
state is three-valued — voted, not voted, **unknown** — never a boolean. A
fresh machine, a discarded cache, a mutation whose response never arrived:
all of them read *unknown* and render neutral. The static catalog site is
always neutral, because it is prerendered and has no identity at build
time.

Unknown is meant to be *resolvable*, not permanent. A client with a
credential asks the forge directly, through grim:

```console
$ printf '%s' "$FORGE_TOKEN" | grim rate ghcr.io/acme/skills/code-review \
    --dry-run --token-stdin --format json | jq .viewer_up
true
```

One read-only query, no mutation, and no confirmation flag — a dry run
posts nothing, so there is nothing to confirm. Nothing else can answer it:
the count in `stats.json` is an aggregate with no identity in it, and
neither the catalog site nor the extension queries a forge on its own.

The invariant is what happens when that query *fails*. It reports
`viewer_up: null` and exits `0` — never `false`. "You have not voted" is a
claim about the forge's state, and a query that did not complete observed
nothing, so grim declines to make it. An operator reading a client's
neutral affordance is seeing an honest *unknown*, not a bug.

## Turn It On {#setup}

Three things have to line up: a `ratings` block in `index.config.json`, a
container on the forge for the threads to live in, and a trusted bot
identity whose posts the tally will believe.

The block is the same on every forge:

| Key | Effect |
|---|---|
| `provider` | `github` or `gitlab`. **Required**, no default — it selects the mutation `grim rate` issues and which environment variables the tally reads |
| `container` | Where threads live. GitHub: a **Discussions category** name. GitLab: a **work item type**, `Issue` or `Task`. **Required**, no default — a default correct on one forge is wrong on the other |
| `createBudget` | Threads created per run. Default `400`, which sits under GitHub's 500-per-hour content-creation cap. A small budget just means more runs to converge |
| `lockThreads` | Lock each thread on creation: votes still count, replies are refused. **Default `true`** — a rating signal without a comment forum to moderate, and an independent hardening of the marker rule |

Two values outside the block are **required once ratings are on**, both
enforced with exit `65` before any forge request:

- **`site`** in [`index.config.json`][branding] — the tally reads the
  previously published sidecar from `<site>/stats.json`. It is deliberately
  not defaulted: the built-in default names the first-party index, and
  seeding from someone else's published ratings would be worse than
  failing.
- **At least one `trustedBots[]` entry carrying an `id`** in
  `index-policy.json`. The marker rule has an account id and no login in
  hand, so a bare-string or id-less entry authorizes nothing.

Unknown keys inside `ratings` are ignored, following the same
forward-compatibility rule `stats.json` itself follows.

After editing the block, re-render the pipeline — the tally job, its
schedule, and the deploy's seed step are all generated from it:

```console
$ npm run ci        # rewrite the workflow files
$ npm run ci:check  # what CI runs — exits 65 on drift
```

### GitHub.com {#setup-github}

Enable **Discussions** on the index repository and create the category
named in `container` (the *announcement* format is a good fit: only
maintainers open threads, which is exactly what the tally does).

```json
{
  "site": "https://index.acme.example",
  "ratings": {
    "provider": "github",
    "container": "Ratings",
    "createBudget": 400,
    "lockThreads": true
  }
}
```

The generated workflow adds a `ratings` job ahead of `build`, an hourly
schedule to run it on, and a seed step in `build`. The job needs one
permission and nothing else:

```yaml
permissions:
  contents: read
  discussions: write        # the only write this pipeline is granted
env:
  GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
```

That is the job's own ephemeral `GITHUB_TOKEN` — repo-scoped, expiring when
the job ends, nothing to rotate. `GITHUB_REPOSITORY` and
`GITHUB_GRAPHQL_URL` are predefined by Actions, so **there is nothing else
to configure**.

Whoever the threads are created *as* must be in `trustedBots[].id`. With
the workflow token that is the `github-actions[bot]` account; with a bot
PAT it is that bot's own account. Read the numeric id from the API rather
than transcribing it, using [`gh`][gh-cli]:

```console
$ gh api /users/github-actions%5Bbot%5D --jq .id
```

**Outside Actions** — a self-hosted cron, a laptop, any non-Actions runner
— nothing is predefined, so all three are set by hand:

```sh
export GRIM_RATINGS_TOKEN=github_pat_...                    # fine-grained PAT
export GITHUB_REPOSITORY=acme/index
export GITHUB_GRAPHQL_URL=https://api.github.com/graphql
npx --no grim-indexer ratings
```

Use a **fine-grained [PAT][gh-pat] scoped to the one repository**, with
`Discussions: read/write` and `Metadata: read`. Leaking it buys an attacker
the ability to manage discussions and toggle upvotes in one repository — it
cannot read code, push, or reach secrets. `GRIM_RATINGS_TOKEN` wins over
`GITHUB_TOKEN` when both are set; an empty value counts as unset.

### GitHub Enterprise Server {#setup-ghes}

**Inside Actions, GHES needs no GHES-specific configuration at all.**
Actions on GHES sets `GITHUB_GRAPHQL_URL` to the instance's own endpoint,
and the tally reads that variable rather than hardcoding `api.github.com` —
so the block above works unchanged on a self-hosted instance.

What you own is the instance, not a setting:

- **Discussions must be enabled** on the repository. GHES has shipped
  Discussions since 3.6; an older instance cannot host ratings.
- **Actions must be available** for the tally job to run in. If it is not,
  use the non-Actions recipe above with `GITHUB_GRAPHQL_URL` pointed at
  `https://ghe.corp.example/api/graphql`.
- The site must be reachable from the runner: the tally seeds itself from
  `<site>/stats.json`, and an unreachable seed **fails the job** by design.

Voters need one thing, covered under [Voting against a private
instance](#voting-host).

### GitLab, SaaS and self-managed {#setup-gitlab}

One recipe covers both tiers. `CI_PROJECT_PATH`, `CI_API_GRAPHQL_URL` and
`CI_SERVER_URL` are all predefined on gitlab.com **and** on a self-managed
instance, so **self-managed needs no extra configuration either**.

Threads are [work items][gl-work-items] rather than discussions, and a vote
is an [emoji reaction][gl-reactions] rather than an upvote. (GitLab renamed
this feature *Award Emoji* to *Emoji Reactions* in 16.0; the GraphQL
mutation is still spelled `awardEmojiToggle`, which is why you will see the
legacy name in API traffic and the current one in the UI.)

```json
{
  "site": "https://acme.gitlab.io/index",
  "ratings": {
    "provider": "gitlab",
    "container": "Issue",
    "createBudget": 400,
    "lockThreads": true
  }
}
```

The operator sets exactly one thing:

```
Settings → CI/CD → Variables → GRIM_RATINGS_TOKEN   (masked, protected)
```

Use a **[project access token][gl-project-token] with `api` scope**. The
threads live in this same project, so nothing broader is needed —
but `read_api` is *not* enough: toggling a reaction is a write. That token's
own account id is what belongs in `trustedBots[].id`.

Four things are worth knowing before you file this and forget it:

- **`CI_JOB_TOKEN` is never read, and is not a fallback.** It inherits the
  triggering user's role, so a thread created with it would be authored by
  a *human* — satisfying the marker rule's author check with exactly the
  identity that rule exists to keep out. A run with no
  `GRIM_RATINGS_TOKEN` exits `65` asking for one rather than degrading.
- **[Token expiry is mandatory][gl-token-expiry]** since GitLab 16.0 — at
  most 365 days. GitLab 17.6 raised that ceiling to 400 days, but only
  behind the `buffered_token_expiration_limit` feature flag, which ships
  **disabled by default** — an instance stays capped at 365 days unless an
  administrator turns it on. Set a rotation reminder when you create the
  token; the tally will start failing on the day it lapses, and R-2 means
  the symptom is *frozen* ratings rather than missing ones.
- **Create the pipeline schedule yourself.** GitLab has no YAML equivalent
  of GitHub's `schedule:` trigger, so the generated job carries an `if:
  $CI_PIPELINE_SOURCE == "schedule"` rule and waits for a schedule that
  exists. Add one under **Settings → CI/CD → Schedules**.
- **Set the `grim-ratings` resource group to `oldest_first`.** The
  generated job declares `resource_group: grim-ratings` to stop two runs
  interleaving, but a resource group's [process mode][gl-resource-group]
  defaults to `unordered` and is **not settable in YAML**. Without
  `oldest_first`, a queued run can overtake the one ahead of it and write a
  stale tally over a fresh one. **There is no UI for this** — set it with
  the [resource groups API][gl-resource-group-api]:
  `PUT /projects/:id/resource_groups/grim-ratings` with
  `process_mode=oldest_first`.

## Casting a Vote {#voting}

The command is the same everywhere — the forge is a property of the index,
not of the invocation:

```console
$ grim rate ghcr.io/acme/skills/code-review          # upvote, with a confirmation
$ grim rate ghcr.io/acme/skills/code-review --remove # retract your own upvote
$ grim rate ghcr.io/acme/skills/code-review --yes    # scripted: no prompt
```

A voter needs a credential for the forge the index tallies on, which is
**not** the credential they publish with. grim looks, in order, at
`GRIM_RATE_TOKEN`, then a host-matched CI token, then the forge CLI's
stored credential ([`gh auth token`][gh-cli] / [`glab auth
token`][glab-cli]), and refuses with exit `80` if none resolves. The full
surface, every flag, and all seven exit codes are in [the command
reference][commands-rate].

### Voting against a private instance {#voting-host}

`github` votes against `api.github.com` and `gitlab` against `gitlab.com`.
Neither default is right for GitHub Enterprise Server or a self-managed
GitLab, so a voter redirects grim with an environment variable:

```sh
export GRIM_RATING_HOST=ghe.corp.example      # or gitlab.corp.example
grim rate acme/skills/code-review
```

Three properties of that variable are load-bearing, and none of them is an
implementation detail:

- **It comes from the voter's own environment and nowhere else.** A
  published `stats.json` carries a provider name, an opaque target and a
  URL — and deliberately **no host**. There is nothing in an index-fetched
  document that can reach this value, so a hostile index cannot redirect
  anyone's credential.
- **Comparison is exact.** The host is IDNA-normalised and ASCII-lowercased,
  the port is part of it, and there is **no suffix matching** whatsoever:
  `evil-github.com` and `github.com.evil.tld` are simply different hosts
  from `github.com`. Redirects are disabled outright on the vote client, so
  a `3xx` can never replay the credential at another host.
- **It applies to whichever provider the index declared**, since it names a
  host rather than a forge. Set it per shell or per instance, not globally
  across a machine that browses both a public and a private index.

One host set is contacted over **plain HTTP** rather than HTTPS: the
loopback forms `localhost`, `127.0.0.1` and `[::1]`, on any port. A server
bound to loopback has no certificate to present, and grim's own acceptance
suite needs a fake forge the real CLI can vote against; the alternative was
a test-only seam in the code that sends credentials, which is worse. The
match is on the whole host after normalisation, never a prefix or suffix,
so `localhost.evil.example` and `127.0.0.1.evil.example` are ordinary
remote hosts and stay on HTTPS.

It is still worth knowing what that means for a token. Point
`GRIM_RATING_HOST` at a loopback port with `GRIM_RATE_TOKEN` exported and
the credential reaches whatever is listening there, in the clear — before,
the TLS handshake failed and it never left the process. Whatever binds that
port is on your own machine, so this is a foot-gun rather than an exposure,
but it is one the previous behaviour happened to prevent. `--token-host`
still refuses a mismatch, and the loopback set is exactly the three forms
above.

A client that pipes a credential in — the [VS Code
extension][vscode] does — can ask grim where the vote would go *before*
authenticating, and can make grim fail closed if it guessed wrong:

```console
$ grim rate acme/skills/code-review --dry-run --format json | jq -r .host
ghe.corp.example
```

Bare `--dry-run` resolves everything and mutates nothing: no credential, no
forge request, so it works offline. Adding `--token-stdin` keeps the
"mutates nothing" half and trades the other one — it consumes the
credential for a single read-only query and reports `viewer_up`, so it
does need the network and exits `81` under `--offline`.

`--token-host <host>` then lets the caller *declare* which host the piped
credential belongs to; a mismatch exits `80` naming both hosts, **before
the token reaches any header**, on the dry-run path exactly as on the
voting one. Details in [the command reference][commands-rate].

## Rolling Back {#rollback}

Deleting the `ratings` block is **half** a rollback. The block is what
generates the tally job — but it also generates the deploy's *seed* step,
and that step is what keeps re-publishing the last `stats.json` it can
fetch. Remove the block without re-rendering, and the pipeline you have
committed keeps carrying the final tally forward, unchanged, at the same
URL, forever.

Both halves, in order:

1. **Delete the `ratings` block** from `index.config.json`.
2. **Re-render and commit the pipeline** — `npm run ci`, then commit the
   changed workflow files. This removes the tally job, its schedule, and
   the seed step in one go. (Skipping it also fails the generated
   `verify-ci` check, which exists to catch exactly this.)

The next deploy then publishes a site with no `stats.json` in it — both
[GitHub Pages][gh-pages] and [GitLab Pages][gl-pages] replace the served
tree wholesale — so the URL starts returning `404` and every client reads
unrated with no warning anywhere. If you deploy some other way, by
rsync or by hand onto a static host that merges rather than replaces,
**delete the published `stats.json` yourself**; nothing else will.

Nothing else has to be undone. The forge threads stay where they are (they
are ordinary discussions or work items, and deleting them is optional), the
votes on them stay visible to their authors, and `grim rate` starts exiting
`65` — *the index publishing this artifact declares no rating provider* —
which is the honest answer.

<!-- external -->
[gh-cli]: https://cli.github.com/manual/gh_auth_token
[gh-discussions]: https://docs.github.com/en/discussions
[gh-pages]: https://pages.github.com/
[gh-pat]: https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/managing-your-personal-access-tokens
[gl-pages]: https://docs.gitlab.com/user/project/pages/
[gl-project-token]: https://docs.gitlab.com/user/project/settings/project_access_tokens/
[gl-reactions]: https://docs.gitlab.com/user/emoji_reactions/
[gl-resource-group]: https://docs.gitlab.com/ci/resource_groups/
[gl-resource-group-api]: https://docs.gitlab.com/api/resource_groups/
[gl-token-expiry]: https://docs.gitlab.com/user/profile/personal_access_tokens/
[gl-work-items]: https://docs.gitlab.com/user/work_items/
[glab-cli]: https://gitlab.com/gitlab-org/cli
[open-vsx]: https://open-vsx.org/

<!-- internal -->
[branding]: ./hosting-an-index.md#branding
[commands-rate]: ./commands.md#rate
[commands-search-sort]: ./commands.md#search-sort
[commands-tui]: ./commands.md#tui
[json-search]: ./json-interface.md#shapes-items
[offline]: ./configuration.md#environment-variables
[self-hosting-git]: ./package-index.md#self-hosting-git
[spec-compiled]: ./package-index.md#spec-compiled

<!-- grimoire -->
[vscode]: https://marketplace.visualstudio.com/items?itemName=grimoire-rs.grimoire
