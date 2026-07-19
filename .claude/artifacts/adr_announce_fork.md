# ADR: Auto-fork the index repository on announce without push access

## Metadata

**Status:** Accepted
**Date:** 2026-07-19
**Deciders:** Michael Herwig (maintainer)
**Tech Strategy Alignment:**
- [x] Decision follows Golden Path in `.claude/rules/product-tech-strategy.md`
      (git + OCI substrate unchanged; forge REST over the existing `reqwest`
      client, git over the existing subprocess seam — no new crate, no new
      infrastructure)
**Domain Tags:** security, integration, api
**Supersedes:** N/A
**Amends:** `adr_grim_publish.md` (D6 report shape — see the re-sync note there)
**Code:** `src/catalog/forge.rs` (`ensure_fork`, `ForkTarget`, forge parsers,
permission probes, `wait_ready`), `src/catalog/index_announce.rs` (fork
orchestration + push), `src/api/publish_report.rs` (`PublishFork`),
`src/error.rs` (`AnnounceError::Fork` classification)

## Context

`grim publish --announce` records published packages in a package-index git
repository: clone, write the `index/<host>/…` pointers, commit on a
deterministic topic branch, push, and open the pull/merge request through
the resolved forge API. The push target was always the **upstream** index.

When the announcing token has no push access to the upstream index, that
push fails — and it fails **after** the packages have already been pushed to
the registry, surfacing as `AnnounceError::Git` → exit 69. The packages
*are* published; only the cross-repository announcement is missing. This is
the exact situation a community contributor announcing to a public index
(`github.com/grimoire-rs/index`) hits: they can read the index and open a
PR from a fork, but cannot push a branch to it directly.

The standard forge workflow for "propose a change to a repo you cannot push
to" is **fork → push branch to the fork → open a cross-repository PR/MR**.
This ADR records the decision to make `grim publish --announce` do that
automatically, and the security guards that keep an automated fork-and-push
safe. It also records two refinements decided this session (GitLab
identity-based reuse, hardened readiness) and the associated hardening
follow-ups.

The feature shipped before this ADR was written (a Principle 7 gap the
`/swarm-review` of `feat/announce-fork` flagged); this record is the
retroactive trail plus the accepted refinements not yet in the shipped code.

## Decision

### D1 — Default-on, opt-out via `[announce] fork`

Auto-fork is **on by default**. `[announce] fork` defaults `true`; setting
`fork = false` forces the upstream push (today's behavior). The flag rides
as `AnnounceRequest::allow_fork`. A fork is **never** attempted without a
forge API token regardless of the flag — a plain-git or tokenless announce
degrades to the upstream push unchanged.

Rationale: the feature is strictly additive — it turns an exit-69 failure
into a merge request. The failure mode it replaces is worse than any of its
own failure modes (all of which degrade back to the upstream push). Default
opt-out would leave the common contributor path broken.

### D2 — Trigger: fork only on a *certain* no-push probe

`ensure_fork` forks **only** when the push-permission probe returns
`Some(false)` — a definite "cannot push":

- GitHub: `.permissions.push == false` on the repo object (`github_can_push`).
- GitLab: max of `project_access` / `group_access` `access_level` below
  Developer (30) (`gitlab_can_push`).

Any **ambiguous** result degrades to the upstream push rather than forking:
an absent permissions block (unauthenticated / scope-limited response), a
failed probe request, an underivable project path, or a permission that
shows push access. Tri-state (`Some(true)` / `Some(false)` / `None`) is
deliberate — a `bool` would collapse "ambiguous" into "cannot push" and fork
when it should not.

Self-fork guard: if the authenticated identity owns the upstream namespace,
forking is impossible (a forge refuses to fork your own repo), so
`ensure_fork` returns `Ok(None)` and pushes upstream.

### D3 — `ensure_fork` contract: `Result<Option<ForkTarget>, AnnounceError>`

- `Ok(Some(target))` — a fork exists and is ready; push the branch there and
  open a cross-repository PR/MR.
- `Ok(None)` — **degrade to the upstream push.** No token, plain forge,
  underivable path, ambiguous or push-capable permission, or self-owned
  upstream. This path is never worse than today's behavior.
- `Err(AnnounceError::Fork { detail })` — forking was **required** (a certain
  no-push) but the fork could not be created, security-verified, or made
  ready. Distinct from a plain push failure: the caller genuinely cannot push
  upstream, so there is no silent fallback that would succeed.

`Ok(None)`-as-degrade is the invariant that keeps the feature strictly
additive: every non-required fork failure lands back on the pre-feature
code path.

### D4 — Parent-verification security guard (create AND reuse)

Before any branch is pushed, the fork's provenance is verified against the
upstream on **both** the create and the reuse path:

- GitHub: the fork's `parent.full_name` must equal the upstream `owner/repo`
  (case-insensitive — GitHub owners are case-insensitive). A missing
  `parent` is rejected.
- GitLab: the fork's `forked_from_project.id` must equal the upstream
  project id. A missing `forked_from_project` is rejected.

Without this guard an existing repository at the conventional fork path that
is a **same-named stranger repository** (not actually a fork of the
upstream) would become a push target — leaking the announce branch, and any
credentials the push carries, to an unrelated owner. The guard turns "same
name" into "verified same lineage".

### D5 — `ForkTarget` fields read from the response body, never composed

Every `ForkTarget` field (`push_url`, `head_owner`, `full_name`, the GitLab
project ids) is read from the fork API **response body**, never composed
from the upstream basename. A fork can be **renamed** (upstream `index`,
fork `grimoire-index`); a `{login}/{basename}` guess would target the wrong
repository. The push URL comes from the API (`clone_url` / `http_url_to_repo`),
the PR head owner from `owner.login` / the namespace of
`path_with_namespace`.

### D6 — GitLab existing-fork reuse becomes identity-based *(session decision — adopted, implemented v1)*

**Shipped today:** on a 409 (fork already exists), GitLab reuse guesses the
fork path as `{authenticated_user}/{basename}` and looks it up
(`gitlab_ensure_fork`, `forge.rs`). This guess fails — exit 69, after the
packages are pushed — for a fork that was renamed, lives in a group
namespace, or was created concurrently under a different path.

**Decided:** replace the basename guess with an identity-based lookup. On a
409:

1. Enumerate `GET /projects/:upstream_id/forks?owned=true&per_page=100`
   (the `owned` filter returns only the authenticated user's forks — a
   user forks a project at most once into their own namespace — so the
   reuser's fork is present regardless of how many forks the upstream has;
   pagination via `Link: rel="next"` is followed as a fallback).
2. Select the fork whose `forked_from_project.id == upstream_id` **and**
   whose namespace belongs to the authenticated user.
3. Poll that fork **by project id** until `import_status == "finished"`.

This reuses the same parent-verification guard (D4) as the create path and
is robust to renamed / concurrently-created forks. The `forks` listing is
the authoritative source of the fork's real path; the guess is not.

**v1 scope:** the namespace match is the authenticated user's **personal**
namespace only. A fork created into a **group** namespace is not reused
(it also fails the D8 `namespace_root == login` push-URL binding, so it
could not be a valid push target regardless). Group-membership reuse — and
the group-membership verification it requires — is a tracked follow-up, not
part of v1.

### D7 — Hardened fork-readiness poll *(session decision — adopted, implemented v1)*

**Shipped today:** `wait_ready` polls a fixed `FORK_POLL_ATTEMPTS` (10)
times spaced `FORK_POLL_INTERVAL` (2s) apart, treating any non-ready
response as "retry" until the attempt budget is exhausted.

**Decided:** harden readiness detection:

- **Per-attempt timeout** so one hung request cannot consume the whole
  budget, plus an **overall wall-clock deadline** so a fork that never
  finishes fails on a bounded schedule rather than an attempt count that is
  only incidentally time-bounded.
- **GitLab `import_status == "failed"` fast-fail** — a failed import will
  never become `finished`; polling it to exhaustion wastes the deadline and
  buries the real cause.
- A **`tracing` progress line** per attempt so a slow fork is observable in
  CI logs rather than looking like a hang.
- **GitHub git-objects readiness:** GitHub's repository *metadata* is
  readable before its *git objects* are provisioned (the fork POST returns
  200 with a `full_name` while `git push` still 404s / "not ready"), and
  provisioning can take minutes. Verify readiness for the push by a single
  **bounded push-retry** on the transient "not ready" failure rather than
  trusting the metadata read alone.

Rationale: the metadata-ready-before-objects-ready gap is documented GitHub
behavior; a poll that only reads repo metadata can report "ready" while the
first push still fails. The readiness signal that matters is "the push
succeeds", so the push path owns one bounded retry.

### D8 — Security hardening of the fork push path *(adopted; implemented)*

The fork push target and the forge client handle attacker-influenceable
data (an API-supplied URL, a redirect target), so:

- **`push_url` must be https-validated before use as a git remote.** The
  push URL arrives from the fork API response and is handed to
  `git push <url>`; it must be confirmed `https://` before use so an
  unexpected transport (`ext::`, `file://`, `ssh://`) cannot be smuggled in
  via a compromised or misbehaving forge response.
- **The forge HTTP client uses a no-redirect policy.** Forge requests carry
  the auth header (`Authorization: Bearer` / `PRIVATE-TOKEN`). `reqwest`
  follows redirects by default and re-attaches headers; a redirect to an
  attacker host would leak the token. The client
  ([`src/catalog/forge.rs`] `client()`) must set
  `redirect::Policy::none()` so a cross-host redirect fails loudly instead
  of forwarding the credential.
- **`git push` restricts protocols and uses `--` end-of-options.** The push
  invocation must place `--` before the URL and branch so an API-supplied
  value that looks like an option cannot be interpreted as one, and should
  constrain the allowed git transports (e.g. `protocol.allow` / a
  transport allowlist) so only the expected `https` transport runs.
- **`authorize` and `resolve` use exhaustive `ForgeKind` matches.**
  `ForgeKind` is a closed internal enum (no `#[non_exhaustive]`, per the
  arch-principles "Internal enum exhaustiveness" convention), so a `_`
  wildcard is avoidable. A wildcard in `authorize` would silently send a
  future third forge kind **unauthenticated** (no header attached); an
  exhaustive match forces every new kind to be classified at compile time.

These follow the same posture already established on the announce **git
transport** side, where the GitLab CI job-token credential helper is
URL-scoped to the gated host so a redirect or submodule fetch cannot draw it
to another host (Clone2Leak / CVE-2024-53858 class — see
`index_announce.rs::job_token_credential_config`). The fork push shares that
host and credential helper, so the helper covers the fork push unchanged.

### D9 — Orphan-fork model and idempotency

A fork created by one run whose later step fails (readiness times out, the
push fails) leaves an **orphan fork** in the authenticated namespace. This
is **benign and self-healing**: the next announce run finds it via the reuse
path (D6), verifies it (D4), and adopts it with `created = false`. No
cleanup / delete-on-failure is attempted — deleting a repository on a
transient failure is more dangerous than leaving an idle fork.

Idempotency holds end to end: the topic branch name is deterministic
(`announce/<ns>-<hash8>` over the rendered pointer set), so a re-announce
**force-updates its own branch** rather than accumulating branches; the
PR/MR create reuses the existing open request on a 422 (GitHub) / 409
(GitLab) instead of erroring. Re-running an announce that previously forked
is therefore safe and convergent.

### D10 — Report shape and Principle 9 additive-compatibility

The fork rides the machine-readable report as an always-present field:

- `AnnounceOutcome::{PullRequest, BranchPushed}` carry
  `fork: Option<AnnouncedFork>` (`{repo, created}`); `None` for an upstream
  push.
- `PublishAnnounce.fork: Option<PublishFork>` (`{repo, created}`) is an
  **always-present-null** JSON field (`null` on an upstream push, populated
  on a fork) — per the `src/api` additive-field policy (no
  `skip_serializing_if`).
- `AnnounceError` is `#[non_exhaustive]`; the new `Fork` variant is additive.
  It classifies as `ExitCode::Unavailable` (69) — a required fork is a
  remote-resource fault, and the packages are already published, so the
  cross-repo announce is the retryable part (`src/error.rs`).

No existing schema field, enum literal, or report consumer changes — the
1.0.0 additive-only freeze (Principle 9) holds. An older consumer that does
not know `fork` sees a new always-present key it can ignore; it never sees a
removed or retyped field.

## Consequences

**Positive:**

- A contributor without push access to the index gets a cross-repository
  PR/MR automatically instead of exit 69 after their packages are already
  published.
- Every failure mode of the fork path (ambiguous permission, no token,
  plain forge, self-owned upstream) degrades to the pre-feature upstream
  push — strictly additive.
- The parent-verification guard makes an automated fork-and-push safe
  against same-named stranger repositories.

**Negative / Risks:**

- Orphan forks accumulate in the authenticated namespace on repeated
  transient failures (benign, adopted on re-run; documented, not
  auto-cleaned).
- The identity-based GitLab reuse (D6) costs an extra `GET …/forks`
  enumeration on the 409 path — acceptable, it runs only when a fork already
  exists.
- The hardening in D8 and the refinements in D6/D7 are **decided and landing
  as tracked follow-ups** — some (e.g. the no-redirect forge client) already
  merged, others in flight — rather than all present in the first-shipped
  feature. Until each lands, its pre-hardening behavior stands: GitLab reuse
  by basename guess (D6), the fixed readiness poll (D7), and any un-merged
  D8 item. This ADR is the authorizing record for those follow-ups; consult
  `src/catalog/forge.rs` for the current landed state.

## Links

- `adr_grim_publish.md` — the `--announce` step and the report envelope this
  extends (its D6 report-shape example is re-synced to the shipped
  `{items, …, announce:{…, fork}}` envelope)
- `adr_push_pull_registry_split.md` — sibling announce/publish decision;
  the additive always-present `pushed_to` field is the report-policy
  precedent the `fork` field follows
- `.claude/rules/quality-security.md` — path-traversal / credential-leak
  guard principles the D8 hardening implements
- `.claude/rules/arch-principles.md` — "Internal enum exhaustiveness"
  convention behind the exhaustive-`ForgeKind`-match decision (D8)
