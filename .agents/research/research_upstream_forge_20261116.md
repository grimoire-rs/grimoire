# Research: upstream refresh — forge (sweep)

## Metadata
**Date:** 2026-11-16 · **Domain:** forge · **Depth:** sweep — never checked
**Triggered by:** /upstream-refresh
**Expires:** 2027-05-16

## Claims

| # | Claim | Ledger location | Old value | New value | Source URL | Verbatim quote | Vendor version |
|---|---|---|---|---|---|---|---|
| 1 | GHES has shipped Discussions since 3.6 | .agents/upstream-watchlist.md:182 (row 1); docs/src/content/docs/ratings.md:253-254 | "GHES has shipped Discussions since 3.6" | unchanged | https://docs.github.com/en/enterprise-server@3.6/admin/release-notes | "GitHub Discussions is available for GitHub Enterprise Server." (3.6 release notes, Community experience). Blog: "It brings more than 60 new features, including: GitHub Discussions : a dedicated home for developing ideas" (https://github.blog/changelog/2022-08-16-github-enterprise-server-3-6-is-now-generally-available/) | GHES 3.6 |
| 2 | GitLab token expiry mandatory since 16.0, 365 days | watchlist:183 (row 2); ratings.md:306-307 | "Token expiry is mandatory since GitLab 16.0 - at most 365 days" | unchanged on the 365-day default and 16.0 origin. Nuance (class a): a self-managed admin can switch the requirement off, so "mandatory" is the default, not absolute | https://docs.gitlab.com/user/profile/personal_access_tokens/ ; https://docs.gitlab.com/update/deprecations/ (raw: https://gitlab.com/gitlab-org/gitlab/-/raw/master/doc/update/deprecations.md#non-expiring-access-tokens) | "By default, new access tokens must have an expiry date. If an expiry date isn't explicitly set during token creation, an expiry date of 365 days from the current date is applied. If an administrator turns off this requirement, new tokens created without an expiry date never expire." Deprecations: "Non-expiring access tokens - Announced in GitLab 15.4 - Removal in GitLab 16.0" | GitLab 19.5 docs |
| 3 | 400-day ceiling from 17.6 behind `buffered_token_expiration_limit`, disabled by default | watchlist:183; ratings.md:307-310 | flag default disabled | unchanged | https://docs.gitlab.com/administration/settings/account_and_limit_settings/ ; https://docs.gitlab.com/user/profile/personal_access_tokens/ | "Maximum allowable lifetime limit extended to 400 days in GitLab 17.6 with a feature flag named buffered_token_expiration_limit . Disabled by default." Admin page: "365 days by default. 400 days, if you enable the buffered_token_expiration_limit feature flag." | 17.6 (flag); docs at 19.5 |
| 4 | Award Emoji renamed Emoji Reactions in 16.0 | watchlist:184; ratings.md:270-273 | confirmed via v17.9 page | unchanged. Current unversioned page https://docs.gitlab.com/user/emoji_reactions/ still has no history block (pruned, as ledger says) | https://docs.gitlab.com/17.9/user/emoji_reactions/ | "Renamed from "award emoji" to "emoji reactions" in GitLab 16.0." | 16.0 |
| 5 | `awardEmojiToggle` present, not deprecated or removed | watchlist:185; rating_provider.rs:694, :703, :1031 | present, undeprecated, no removed-items entry | unchanged. Removed-items log lists only 17.0, 16.0 and 15.0 removals; none is `awardEmojiToggle`. Reference (master) lists the mutation with no Deprecated line | https://docs.gitlab.com/api/graphql/removed_items/ ; https://gitlab.com/gitlab-org/gitlab/-/raw/master/doc/api/graphql/reference/mutations.md | Reference: "## `awardEmojiToggle` ... `awardableId` AwardableID! Global ID of the awardable resource. ... `name` String! Emoji name. ... `toggledOn` Boolean! Indicates the status of the emoji." Removed-items: "GraphQL is a versionless API, unlike the REST API. Occasionally, items have to be updated or removed from the GraphQL API." | GitLab 19.5 (current) |
| 6 | Ledger line refs for the hard-coded mutation name | watchlist:185 | `rating_provider.rs` `:592`, `:601`, `:843`, widget `:549`, `:580` | drifted (class a): document literal `:693-694`, response-key lookup `:703`, write-mutation guard list `:1031`, `awardEmoji` widget in `viewer_state_document` `:518-520`, `gitlab_viewer_state` `:547-554`. Same drift for ratings.md line numbers in the other rows (247 -> 253-254, 300 -> 306-310, 264-267 -> 270-274, 311-317 -> 317-325) | repo files (no upstream URL) | n/a, local line check by `/usr/bin/grep -n` | n/a |
| 7 | `grim-ratings` resource group `process_mode` has no UI, API only | watchlist:186; ratings.md:317-325 | "Wrong, doc corrected 2026-08-19"; flip back when #436986 ships | unchanged. Docs still say API only; issue #436986 still open (updated 2026-10-11) | https://docs.gitlab.com/ci/resource_groups/ ; https://gitlab.com/gitlab-org/gitlab/-/issues/436986 | "To change the process mode of a resource group, you must use the API and send a request to edit an existing resource group by specifying the process_mode : unordered oldest_first newest_first newest_ready_first" | GitLab 19.5 docs |
| 8 | Code: GitLab `workItem(id: WorkItemID!)` query (document at rating_provider.rs:518) | rating_provider.rs:518 | not in ledger | unchanged in schema, but the query is still marked experimental upstream (watch item, no change needed): `workItem` is "Status: Experiment" since 15.1; no removal or deprecation notice | https://gitlab.com/gitlab-org/gitlab/-/raw/master/doc/api/graphql/reference/queries.md (`## workItem`) | "Status: Experiment ... Introduced as an experiment in GitLab 15.1. ... Find a work item. ... `id` WorkItemID! Global ID of the work item." | GitLab 19.5 |
| 9 | Code: `WorkItemWidgetAwardEmoji.awardEmoji(first:100){pageInfo{hasNextPage} nodes{name user{id}}}` | rating_provider.rs:519-520 | not in ledger | unchanged: widget and field exist; `AwardEmoji` has `name` and `user: UserCore!`; connection takes standard pagination args | https://gitlab.com/gitlab-org/gitlab/-/raw/master/doc/api/graphql/reference/objects.md | "`awardEmoji` AwardEmojiConnection Emoji reactions on the work item. This field is a connection and accepts the four standard pagination arguments: `before`, `after`, `first`, `last`." ; "`user` UserCore! User who awarded the emoji." | GitLab 19.5 |
| 10 | Code: GitLab `currentUser` query | rating_provider.rs:432, :518 | not in ledger | unchanged: `currentUser` exists, no deprecation shown | https://gitlab.com/gitlab-org/gitlab/-/raw/master/doc/api/graphql/reference/queries.md | "## `currentUser` Get information about current user. **Returns:** CurrentUser" | GitLab 19.5 |
| 11 | Code: GitHub `addUpvote` / `removeUpvote` with `subjectId`, payload `subject { upvoteCount viewerHasUpvoted }`, `Discussion` Votable | rating_provider.rs:620, :624, :512 | not in ledger | unchanged: both mutations exist in the public schema; input takes a Discussion or DiscussionComment node id; `Discussion implements ... Votable`; `viewerHasUpvoted: Boolean!` exists; no GraphQL breaking-change entry mentions upvote | https://docs.github.com/public/fpt/schema.docs.graphql ; https://docs.github.com/en/graphql/overview/breaking-changes | "The Node ID of the discussion or comment to upvote." ; "Whether or not the current user has already upvoted this subject." ; "type Discussion implements Closable & Comment & ... & Votable" | GitHub.com (fpt) schema fetched 2026-11-16 |
| 12 | Code: GitHub `viewer { login databaseId }` | rating_provider.rs:431 | not in ledger | unchanged: `viewer: User!` exists in the public schema (field presence only; `databaseId` not separately grepped) | https://docs.github.com/public/fpt/schema.docs.graphql | "viewer: User!" (Query type) | GitHub.com (fpt) |
| 13 | Code/docs: GHES serves GraphQL from `/api/graphql`; GHE.com and github.com from `api.<host>/graphql` | rating_provider.rs:296-311; ratings.md:256-257 | not in ledger | unchanged in the changelog scan: GHES 3.22 release notes mention GraphQL only for performance and a bug fix, no endpoint change | https://docs.github.com/en/enterprise-server@latest/admin/release-notes | "GitHub Enterprise Server now uses fewer CPU resources when serving REST and GraphQL API requests by skipping unused MySQL query backtrace collection." | GHES 3.22 |

## Verifier sign-off

none required — no code landed in this run.

## Routing

- #2 GitLab token expiry mandatory since 16.0, 365 days → (a) ledger or doc text fix
- #4 Award Emoji renamed Emoji Reactions in 16.0 → (a) ledger or doc text fix
- #5 awardEmojiToggle present, not deprecated or removed → (a) ledger or doc text fix
- #6 Ledger line refs for the hard-coded mutation name → (a) ledger or doc text fix
- #7 grim-ratings resource group process_mode has no UI, API only → (a) ledger or doc text fix
- #8 Code: GitLab workItem(id: WorkItemID!) query (document at... → (a) ledger or doc text fix (new watchlist row)
- #9 Code: WorkItemWidgetAwardEmoji.awardEmoji(first:100){pageInfo{hasNextPage}... → (a) ledger or doc text fix
- #10 Code: GitLab currentUser query → (a) ledger or doc text fix
- #11 Code: GitHub addUpvote / removeUpvote with subjectId, payload subject {... → (a) ledger or doc text fix
- #12 Code: GitHub viewer { login databaseId } → (a) ledger or doc text fix
- #13 Code/docs: GHES serves GraphQL from /api/graphql; GHE.com and github.com from... → (a) ledger or doc text fix

## Issue drafts

none

## Friction

- dropped (unsourced): GHES endpoint path `/api/graphql` and `GITHUB_GRAPHQL_URL` set by Actions on GHES (ratings.md:246-257, rating_provider.rs:300) — not fetched; no feed entry contradicts it. Only the generic forming-calls page cited in code was not re-read.
- dropped (unsourced): GHES schema carrying `addUpvote`/`removeUpvote` at 3.22 — `https://docs.github.com/public/ghes-3.22/schema.docs.graphql` returned 404; only the github.com (fpt) schema was checked (row 11).
- dropped (unsourced): GitHub 500-per-hour content-creation cap (ratings.md:158) — not checked.
- dropped (unsourced): GitLab `CI_API_GRAPHQL_URL`, `CI_SERVER_URL`, `CI_PROJECT_PATH` predefined variables (ratings.md:266-269) — not checked.
- dropped (unsourced): GitHub `databaseId` field on `User` — not grepped.
- not reached: ratings.md claim that a project access token with `api` scope is needed and `read_api` is not enough (:296-299).
- not reached: ratings.md claim `CI_JOB_TOKEN` inherits the triggering user's role (:301-305).
- not reached: GitHub fine-grained PAT / Actions `GITHUB_TOKEN` claims (ratings.md:184-242).
- not reached: Indexer README re-check noted in the ledger (outside this repo).
- feed: https://about.gitlab.com/releases/categories/releases/ resolved to the docs.gitlab.com release-notes index, so the GitLab version came from the releases.xml RSS feed.
- feed: newest GHES 3.22 (3.22.4); GitLab 19.5, cursor found no

## Skill changes

Host caps concurrent subagents at 20; the researcher built its own claim list; replies are a short summary with the table in a file. Fixed in this run's chore(skills) commit.
