# ADR: The index declares its rating host; injected credentials are bound to it

## Metadata

**Status:** Accepted
**Date:** 2026-09-01
**Deciders:** Michael Herwig + Claude
**Beads Issue:** N/A
**Related PRD:** N/A
**Tech Strategy Alignment:**
- [x] Decision follows Golden Path in `.claude/rules/product-tech-strategy.md` (no new dependency)
**Domain Tags:** security, integration
**Supersedes:** the endpoint-resolution clause of [`adr_artifact_ratings.md`](./adr_artifact_ratings.md) D13
**Superseded By:** N/A

## Context

`grim rate` resolved the forge host from a built-in per-provider default
(`github` ⇒ `api.github.com`, `gitlab` ⇒ `gitlab.com`) plus `GRIM_RATING_HOST`,
an environment variable read from the voter's own process and nowhere else.
That was D13's deliberate choice: index-supplied host data was named as a
token-exfiltration vector and forbidden outright.

The consequence is that **every self-hosted deployment is misconfigured by
default, and silently**. The index operator runs their own GitLab, publishes
`stats.json` with `providers.rating = "gitlab"`, and every consumer's vote
still resolves to gitlab.com until each of them individually exports the
variable. One team, one index, N machines that each have to be configured by
hand, with no error when they are not — the vote simply goes somewhere else.
Reported from the VS Code extension: a corporate user clicked upvote and the
vote targeted gitlab.com.

The host is a property of the index that hosts the threads, not of the machine
that reads it.

## Decision Drivers

- A self-hosted index must work out of the box for a consumer who configured
  nothing but the `index =` locator.
- No credential may reach a host the user has not, directly or transitively,
  agreed to.
- The `--dry-run` handshake ([C-022](../plans/plan_artifact_ratings.md)) stays
  the client's single source of truth for the host, and must get *more*
  informative, not less.
- Additive on the wire: an older grim meeting a newer sidecar keeps working.

## Considered Options

### Option 1: `[[registries]] rating_host` — the consumer's own config declares it

**Description:** A new optional key on the registry entry that already names
the index. The trust anchor is the user's committed `grimoire.toml`, which
[`adr_artifact_trust_model.md`](./adr_artifact_trust_model.md) already treats
as the trust gesture.

| Pros | Cons |
|------|------|
| Zero new trust surface — literally D13's "the user's own config" | The index operator cannot declare it for a consumer; every consumer edits config |
| No sidecar change, no indexer change, no origin logic | Keeps the "configure it by hand" shape the issue exists to remove, only moved from env to file |
| One edit per team rather than per machine | |

### Option 2: `providers.rating_host` trusted on an origin match

**Description:** The sidecar declares the host; grim accepts it only when it
matches the host that served `stats.json`.

| Pros | Cons |
|------|------|
| A hostile index cannot name an unrelated host | **Declines in the case it exists for.** Ratings ride the HTTP index transport only, so a self-managed instance serves its index from GitLab Pages — and GitLab mandates a Pages domain distinct from the instance host. `group.pages.corp.example` never matches `gitlab.corp.example`, and often is not even a suffix relation |
| No new refusal paths | The blanket per-registry opt-in becomes the actual mechanism, making the origin check dead weight |
| | Relaxing to a registrable-domain match needs a public-suffix list, a dependency and a data file for one comparison |

### Option 3: `providers.rating_host` plus a credential-class gate

**Description:** The sidecar declares the host and grim trusts it, but an
*injected* credential may only reach an index-declared host when the caller
names that host with `--token-host`.

| Pros | Cons |
|------|------|
| Works for every deployment shape, Pages included — no origin relationship assumed | New refusal path, and one more thing a piping client must get right |
| D13's own argument carries most of it: the host-matched rungs already resolve nothing for an attacker host | Requires a companion change in `grimoire-rs/indexer` |
| The residual exposure is exactly two credentials, and both are closed explicitly | |

## Decision Outcome

**Chosen Option:** Option 3.

**Rationale.** D13's core claim survives scrutiny: grim's credential ladder
resolves a credential *for the host it is about to contact*
(`forge::ci_token_for`, `gh`/`glab auth token --hostname`), so an index naming
an attacker host resolves nothing and the request goes out bare or fails. What
D13 did not separate is that two credentials are **not** host-bound —
`GRIM_RATE_TOKEN`, which is host-agnostic by construction, and `--token-stdin`,
where the caller injects a credential grim never looked up. That is the entire
exposure, and it is small enough to gate directly rather than to close by
withholding the host from everyone.

Option 2 was rejected on evidence, not on taste: the origin check declines in
the primary self-hosted shape, so it would have shipped a feature that does not
fire where it is needed. Option 1 is sound and was the runner-up; it was
rejected because it keeps the per-consumer configuration step that motivated
the issue.

### Rules

**Provenance.** A resolved host is either `default` (the built-in per-provider
value) or `index` (declared by the sidecar and accepted). There is no third
source — `GRIM_RATING_HOST` is removed by this ADR.

**Accepting a declared host.** `providers.rating_host` is host authority only:
no scheme, no path, no userinfo, port significant. It is accepted when

1. `rating_provider::normalize_host` accepts it, and
2. it is not a loopback form unless the index locator is itself loopback.

Rule 2 is the one guard that survives from Option 2, kept because it is two
lines and closes a distinct class: `graphql_endpoint` speaks plain HTTP to the
loopback set, so a remote index declaring `127.0.0.1:8080` would aim a
credential at a local port in the clear. A rejected declaration degrades to the
default at `debug`, exactly as an unrecognised `providers.rating` degrades to
"readable, not writable".

**Binding injected credentials.** Against an `index`-provenance host, a run that
supplies `--token-stdin` or has `GRIM_RATE_TOKEN` set must also pass
`--token-host`; the declared host is compared to the resolved one under the
unchanged exact rules. A run that does not exits `80` **before the credential is
read**, and never falls through to the host-matched rungs — silently voting as
a different identity is the failure C-006 already refuses for empty
`--token-stdin`. Against a `default` host nothing changes.

**Telling the client which it got.** `RateReport` gains `host_source`
(`"default"` / `"index"` / `null`). It is not cosmetic: `"index"` is precisely
when a piping client must pass `--token-host`, and it is what lets a consent
dialog say the destination is one the index chose.

### Consequences

**Positive:**
- A self-hosted index works for a consumer who configured only the locator.
- The host stops being per-machine state that nothing validates.
- The `--dry-run` handshake gains the one fact a client could not otherwise
  learn — whether the destination was chosen by the index.

**Negative:**
- Two breaking changes, both pre-1.0 and both release-noted: `GRIM_RATING_HOST`
  is removed, and `--token-host` no longer requires `--token-stdin` (it must be
  usable to declare the host for `GRIM_RATE_TOKEN`), which deletes one
  documented `64` condition.
- A scripted corporate voter using `GRIM_RATE_TOKEN` must add `--token-host`.
  That is the point of the gate, but it is a new step.
- The indexer must emit the key before any of this fires for a real index.

**Risks:**
- *A compromised index redirects a vote.* Mitigated to: no host-matched
  credential resolves for the attacker host, and an injected one is refused
  unless the caller names that exact host. The residue is a caller that
  declares an attacker host it was told to declare — indistinguishable from a
  caller instructed to send a token anywhere, and outside
  [`adr_artifact_trust_model.md`](./adr_artifact_trust_model.md)'s boundary.
- *A hostile index aims a credential at loopback.* Closed by rule 2 above.

## Technical Details

```jsonc
// <index base>/stats.json — additive, no schema_version bump
{
  "schema_version": 1,
  "providers": {
    "rating": "gitlab",
    "rating_host": "gitlab.corp.example"   // host authority only; port significant
  },
  "entries": { }
}
```

```text
resolve:   providers.rating_host (accepted)  >  built-in provider default
gate:      source == index && (--token-stdin || GRIM_RATE_TOKEN)
                            && --token-host absent   =>  exit 80
report:    { …, "host": "gitlab.corp.example", "host_source": "index" }
```

`RatingSummary` carries `host` per entry for the same reason it carries
`provider`: nothing in the catalog layout guarantees one cache file holds rows
from a single index build, and `grim rate` needs the destination for the
specific row it resolved.

## Validation

- [x] Security review — the credential-class gate and the loopback rule are the
      two findings this ADR exists to record.
- [x] Additive on the wire (sidecar, cache, report); the two breaks are named
      above and release-noted.
- [ ] Acceptance suite proves the gate refuses before any request reaches the
      fake forge.

## Links

- [`adr_artifact_ratings.md`](./adr_artifact_ratings.md) — the ratings feature; D13 is the superseded clause
- [`adr_artifact_trust_model.md`](./adr_artifact_trust_model.md) — the trust boundary this decision sits inside
- [`plan_artifact_ratings.md`](../plans/plan_artifact_ratings.md) — C-006, C-007, C-022, C-023
- [grimoire-rs/grimoire#110](https://github.com/grimoire-rs/grimoire/issues/110)

---

## Changelog

| Date | Author | Change |
|------|--------|--------|
| 2026-09-01 | Michael Herwig + Claude | Initial decision |
