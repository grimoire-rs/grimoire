# Research: Rating Backends for the Package Index

<!--
Owner: hex-architect run 2026-08-17 (Discover input, pre-Research)
Handoff to: architect (ADR), /hex-plan
Source: design conversation 2026-08-17 — captured so downstream workers do not re-derive it.
-->

## Metadata

**Date:** 2026-08-17
**Domain:** packaging / cli / security
**Triggered by:** [#82](https://github.com/grimoire-rs/grimoire/issues/82) — Ratings for catalog artifacts
**Expires:** 2027-02-17 (re-verify forge APIs and rate limits)

## Direct Answer

Ratings ship as a **static aggregate in the index** (`ratings.json`), with the **write path
delegated to whichever forge hosts the index** — GitHub Discussions or GitLab work items.
No service is operated, no database, no OAuth application, no new uptime dependency. The
provider is a data-level seam, so a custom rating service can replace the forge later
without touching any client.

## Landscape — what exists off the shelf

There is **no generic "rating microservice"**. The category does not exist as a
deployable component. What exists, and why each was rejected:

| Option | Shape | Verdict |
|---|---|---|
| [Remark42](https://remark42.com/docs/manuals/kubernetes/) | Comment engine, Go, [Helm chart](https://artifacthub.io/packages/helm/groundhog2k/remark42), voting, custom OAuth2 providers | **Rejected** — closest drop-in, but it is a comment system bent into a rating field; adds a service + DB + OAuth client to a stack with zero servers |
| [Fider](https://github.com/getfider/fider) | Feedback board, Go + Postgres, OIDC/Azure AD, Docker/K8s, AGPL open-core since v0.33 | **Rejected** — voting works, but the "ideas board" model is wrong-shaped for one board per artifact |
| [Open VSX](https://www.eclipse.org/legal/open-vsx-registry-faq/) | Self-hostable extension registry **with Ratings & Reviews built in**, EPL-2.0 | **Rejected** — the closest precedent, but adopting it means adopting their registry, not adding ratings to ours |
| Backstage rating/scorecard plugins | Catalog entity scoring | **Rejected** — only viable for orgs already running Backstage |
| [giscus](https://github.com/giscus/giscus) | GitHub Discussions as the database, thousands of sites | **Pattern adopted, tool not used** — we need counts, not a comment UI; its GHE self-hosting path is where people get stuck |

**Prior art in package registries:** crates.io, npm, and PyPI have **no ratings at all**.
crates.io ranks by relevance + download count and deliberately avoids quality scores; its
[default-ranking RFC](https://rust-lang.github.io/rfcs/1824-crates.io-default-ranking.html)
and later research found people judge crates by *documentation quality*. Ratings exist in
*extension marketplaces* (VS Code Marketplace, Open VSX), not package registries. This is
the strongest argument for keeping the mechanism cheap and reversible.

## Forge API facts (verified 2026-08-17)

### GitHub Discussions

- Repository discussions are **GraphQL only** — no REST.
  [Guide](https://docs.github.com/en/graphql/guides/using-the-graphql-api-for-discussions).
- **Upvotes are first-class**: `upvoteCount` on discussions and top-level comments,
  sortable. Mutations `addUpvote` / `removeUpvote`. Distinct from `reactionGroups`
  (emoji), which require nested nodes and cost more.
- Categories have formats: open-ended, Q&A, **announcement** (only maintainers create
  top-level posts — the mechanism that makes threads bot-owned), poll.
- Moderation is inherited: lock, delete, convert to issue, report abuse, per-category
  moderators.
- **GHES**: Discussions shipped in
  [3.6](https://github.blog/news-insights/product-news/github-discussions-is-now-available-on-github-enterprise-server/);
  the GraphQL guide is published per-GHES-version. Requires Discussions enabled on the
  repo and Actions available for the tally job.
- **Rate limits** ([docs](https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api)):
  primary 5,000 points/hour; secondary **80 content-creating requests/minute, 500/hour**;
  100 concurrent requests; 2,000 points/minute on GraphQL.

### GitLab work items

- GitLab has **no Discussions object** — "discussions" in their API means comment threads.
  Terminology trap.
- The instance in question has issues enabled **as work items**.
- Work items expose an **AwardEmoji widget** carrying `upvotes` / `downvotes`
  ([widget docs](https://labs.onb.ac.at/gitlab/help/development/work_items_widgets.md)).
  Mutation `awardEmojiToggle` returns `toggledOn` — one mutation covers both directions.
  Also reachable via `workItemUpdate` with `awardEmojiWidget: { action: TOGGLE }`.
- The **REST issues API still works but is long-term deprecated** — supported, no removal
  date, frozen to new features
  ([REST deprecations](https://docs.gitlab.com/api/rest/deprecations/)). Build on the
  work items GraphQL API instead.
- Award emoji also work on **project snippets**
  (`/projects/:id/snippets/:snippet_id/award_emoji`, snippet **ID** not IID) — the fallback
  if work items are ever unavailable. Unauthenticated read of public awardables since 15.1.
- GitLab refuses award emoji on your own issue → **self-rating filter is native**.
- Widget availability is **per work item type**; Issue and Task carry AwardEmoji.

## Identity and credential findings

- **OIDC, not SAML.** The requirement is one-vote-per-human, not identity. SAML has no
  device flow and is browser/XML-centric — wrong for a CLI. Every corporate IdP (Entra,
  Okta, Keycloak, Ping) speaks OIDC on the same tenant.
- **VS Code built-in providers**: `github`, `github-enterprise` (configured by the existing
  `github-enterprise.uri` setting), `microsoft`. Known bug where `github-enterprise.uri` is
  ignored and auth redirects to github.com
  ([#3481](https://github.com/microsoft/vscode-pull-request-github/issues/3481)) —
  workaround is explicit sign-in via Command Palette.
- **GitLab has no built-in VS Code provider**, but the official **GitLab Workflow**
  extension registers one (`gitlab`, OAuth2 PKCE):
  `getSession('gitlab', ['api','read_user'], { createIfNone: true })`
  ([MR](https://gitlab.com/gitlab-org/gitlab-vscode-extension/-/merge_requests/556/commits)).
  Reusing it means **no second login and no OAuth app registration**.
- **GitLab OAuth** ([docs](https://docs.gitlab.com/api/oauth2)): PKCE for public clients;
  Device Authorization Grant since 17.1 (needs `device_code` grant enabled; 17.9+ for
  CLI-style use). Any OAuth path requires an application registered at
  `/user_settings/applications` — an admin step. **PAT is therefore the documented
  default**, OAuth the upgrade.
- **GitLab PATs**: expiry mandatory since 16.0, max 365 days (400 on 17.6+), Ultimate
  admins can lower it. Creation page accepts prefill:
  `?name=Grimoire&scopes=api`
  ([docs](https://archives.docs.gitlab.com/16.11/ee/user/profile/personal_access_tokens.html)).
  Award emoji is a write → needs `api`, not `read_api`.
- **glab**: stores credentials in `~/.config/glab-cli/config.yml` or the OS keyring;
  `glab auth login --hostname`, `--stdin` for token input
  ([docs](https://docs.gitlab.com/cli/authentication/)).
- **Existing grim precedent**: `grim login --password-stdin` already reads a credential
  from stdin (`src/command/login.rs:45`, read path `:156`, documented in
  `docs/src/authentication.md`). The announce path already implements a host-matched token
  ladder (`GRIM_ANNOUNCE_TOKEN` > `GH_TOKEN`/`GITHUB_TOKEN`/`GITLAB_TOKEN` host-matched,
  plus `CI_JOB_TOKEN` presence check) — the rating ladder mirrors it.

## Performance analysis

At the current **~200 artifacts**, and projected:

| Concern | At 200 | At 10,000 |
|---|---|---|
| Tally (paginated 100/page) | 2 requests | 100 requests, ~100 GraphQL points of 5,000/hr, ~30–60s sequential |
| `ratings.json` size | negligible | ~1.5MB raw / ~300KB gzipped — **mitigated by omitting zero-vote entries** (~75KB if 5% have votes) |
| Thread creation cold start | minutes | **~20 hours** at 500 content-creations/hour — the real bottleneck; throttle across runs |
| Vote write | 1 request per click | 1 request per click |

Using the scalar `upvoteCount` rather than counting reaction nodes avoids nested-node
multiplication and keeps tally cost at ~1 point/page.

**Ceiling:** forge-as-database is comfortable to roughly 1–2k artifacts. Past that, the
20-hour backfills and an unusable Discussions tab are the signal to migrate to a real
service — which the provider seam prices in.

## Design patterns adopted

- **Forge as database** (giscus pattern) — the forge supplies storage, identity,
  moderation, and abuse reporting; the project operates nothing.
- **Static read path / live write path** — reads are a cached static artifact so browsing
  never depends on an uptime we do not control and works offline; writes are live, optional,
  and allowed to fail without degrading reads.
- **Derived-not-stored mapping** — the bot embeds `<!-- grim-ref: <ref> -->` in the thread
  body, so `ref → target` is reconstructed from the forge on every run. No back-push into
  the publish path, no forge-specific id in a frozen contract.
- **Opaque handles** — `target` (machine) and `url` (human) are strings no client parses
  or constructs, which is what makes the provider swappable.
- **Absent is first-class** — a missing file, entry, or field means "no rating", never an
  error. This one invariant carries offline use, older indexes, and non-participating
  indexes.

## Key findings

1. No off-the-shelf rating service exists; every candidate is a comment or feedback product
   bent into the role, and each adds a service, a DB, and an OAuth client to a stack that
   currently has none.
2. Package registries deliberately avoid ratings; extension marketplaces have them. This
   argues for the cheapest reversible mechanism, not the richest.
3. Both target forges expose a native upvote primitive and a toggle mutation, so the two
   providers converge on nearly identical client shapes.
4. Identity is free on both forges — GitHub via VS Code built-ins, GitLab via the GitLab
   Workflow extension's registered provider — provided we reuse sessions rather than
   competing with them.
5. The scaling bottleneck is thread *creation* (500/hour), not tallying or serving.

## Recommendation

Forge-backed ratings, both providers from day one, with the seam expressed as data
(`ratings.json` schema, opaque `target`/`url`, `provider.kind` discriminator,
absent-is-first-class) rather than as an abstraction layer. Record the scale ceiling with
measurable revisit triggers rather than an open-ended promise.

## Sources

| Source | Type | Relevance |
|---|---|---|
| [Discussions GraphQL guide](https://docs.github.com/en/graphql/guides/using-the-graphql-api-for-discussions) | Docs | Upvotes, mutations, GraphQL-only |
| [GHES 3.6 announcement](https://github.blog/news-insights/product-news/github-discussions-is-now-available-on-github-enterprise-server/) | Blog | Discussions availability on GHES |
| [GitHub rate limits](https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api) | Docs | 80/min, 500/hour content creation |
| [Work item widgets](https://labs.onb.ac.at/gitlab/help/development/work_items_widgets.md) | Docs | AwardEmoji widget, upvotes |
| [GitLab REST deprecations](https://docs.gitlab.com/api/rest/deprecations/) | Docs | Issues API deprecation status |
| [GitLab award emoji API](https://archives.docs.gitlab.com/15.11/ee/api/award_emoji.html) | Docs | Snippet fallback, awardables |
| [GitLab OAuth 2.0](https://docs.gitlab.com/api/oauth2) | Docs | PKCE, device grant, app registration |
| [GitLab PATs](https://archives.docs.gitlab.com/16.11/ee/user/profile/personal_access_tokens.html) | Docs | Expiry limits, prefill URL |
| [glab authentication](https://docs.gitlab.com/cli/authentication/) | Docs | Credential storage, hostname flag |
| [GitLab Workflow auth provider](https://gitlab.com/gitlab-org/gitlab-vscode-extension/-/merge_requests/556/commits) | MR | `getSession('gitlab', …)` |
| [VS Code API](https://code.visualstudio.com/api/references/vscode-api) | Docs | Auth providers, `getSession` |
| [`github-enterprise.uri` bug](https://github.com/microsoft/vscode-pull-request-github/issues/3481) | Issue | GHES auth caveat |
| [Remark42 Helm](https://artifacthub.io/packages/helm/groundhog2k/remark42) | Chart | Rejected option |
| [Fider self-hosting](https://owrbit.com/hub/self-host-fider-docker-open-source-feedback-tool/) | Guide | Rejected option |
| [Open VSX FAQ](https://www.eclipse.org/legal/open-vsx-registry-faq/) | Docs | Rejected option, closest precedent |
| [crates.io ranking RFC](https://rust-lang.github.io/rfcs/1824-crates.io-default-ranking.html) | RFC | Registries avoid ratings |
| [giscus](https://github.com/giscus/giscus) | Repo | Forge-as-database pattern |
