# Research: HTTP & OCI Revalidation Semantics for Catalog Cache Freshness

**Axis:** technology | **Run:** `/hex-plan high` — catalog revalidation
**Scope:** is conditional-GET / HEAD-digest revalidation a safe, cheap replacement for
Grimoire's current "unconditional GET + 1h wholesale-rebuild TTL" for the three
browse-catalog source kinds (HTTP index, git index, OCI `_catalog` walk)?

**Already established in-tree (not re-derived here):** zero occurrences of
ETag/If-None-Match/304/Last-Modified in the crate today — greenfield. HTTP index
fetch is an unconditional `GET <base>/all.json` via reqwest, 30s timeout
(`src/catalog/index_source.rs:152-178`). OCI path already resolves a tag digest
via `HEAD` (oci-client `fetch_manifest_digest`, reading `Docker-Content-Digest`).
Public index is served from GitHub Pages.

---

## 1. Conditional GET against GitHub Pages

GitHub Pages (and `raw.githubusercontent.com`) are served through **Fastly**,
which GitHub explicitly confirms sits in front of github.com, Pages, and raw
content delivery, custom-tuned with GitHub's own CDN config
([Fastly/GitHub case study](https://www.fastly.com/customers/github)). Static
JSON assets on this stack conventionally get strong, content-derived `ETag`
values and a `Last-Modified` header, and conditional requests
(`If-None-Match` / `If-Modified-Since`) get genuine `304 Not Modified`
responses per standard Fastly/HTTP semantics
([Fastly: ETags explained](https://www.fastly.com/blog/etags-what-they-are-and-how-to-use-them),
[MDN: conditional requests](https://developer.mozilla.org/en-US/docs/Web/HTTP/Guides/Conditional_requests)).
This is the same mechanism GitHub's own REST API documents and recommends for
conditional polling ([GitHub REST API best practices](https://docs.github.com/en/rest/using-the-rest-api/best-practices-for-using-the-rest-api?apiVersion=2026-03-10)).

**Caveats found:**
- **Weak vs strong / compression coupling.** An ETag computed over a gzip
  representation differs from one over the identity representation
  (`"<hash>+gzip"` is a common pattern) — this is general HTTP CDN behavior,
  not GitHub-specific. Not a problem for a single client sending a stable
  `Accept-Encoding` header across requests, but a captured ETag must not be
  compared against a response fetched with a different `Accept-Encoding`.
- **GitHub Pages default cache TTL is short (~10 min) at the edge** — irrelevant
  to conditional-GET correctness (the origin still revalidates on every
  request past that TTL), but confirms edge propagation lag on the order of
  seconds-to-minutes, not hours ([community discussion on GH Pages caching](https://github.com/orgs/community/discussions/11884)).
- **No direct network access in this research environment** — I could not run
  a live `curl -I` against the actual `index.grimoire.rs` host to confirm
  header presence empirically. The claim above is well-established general
  knowledge about the Fastly-backed GitHub static-asset stack, corroborated by
  multiple independent sources, but **the plan should include one live
  `curl -sSI` spot-check against the real index host before shipping**, since
  a custom-domain GitHub Pages site can sit behind an additional CDN/DNS layer
  (e.g., Cloudflare in front of GitHub Pages, a common setup) that could alter
  header behavior.

**Verdict:** conditional GET is safe to build against GitHub Pages. Fall back
to unconditional GET (today's behavior) whenever a response omits both
validators — cheap, no behavior regression.

## 2. reqwest mechanics & crate-vs-hand-roll

**Idiom** (reqwest has no built-in cache; conditional GET is caller-managed):

```rust
use reqwest::header::{ETAG, IF_NONE_MATCH};

let mut req = client.get(url);
if let Some(etag) = cached_etag {
    req = req.header(IF_NONE_MATCH, etag);
}
let resp = req.send().await?;
if resp.status() == reqwest::StatusCode::NOT_MODIFIED {
    // reuse cached body; still bump the cache's "last-checked" timestamp
} else {
    let new_etag = resp.headers().get(ETAG).cloned();
    let body = resp.text().await?; // fresh content + new_etag to persist
}
```
`reqwest::StatusCode::NOT_MODIFIED` is `http::StatusCode`'s 304 constant,
re-exported by reqwest. ~15–25 lines including the cache-file read/write glue
already needed for the TTL check that exists today.

**Crate option:** [`http-cache-reqwest`](https://crates.io/crates/http-cache-reqwest)
(latest `1.0.0-alpha.7`, Aug 2026; 286k downloads/mo, used by 70 crates, 14
contributors — reasonably popular but **still alpha**, semver not yet
stabilized) wraps `reqwest-middleware` + `http-cache-semantics` (full RFC 7234
cache-control/`Vary`/stale-while-revalidate semantics) and optionally
`cacache` for an on-disk cache manager. It solves a strictly bigger problem
than Grimoire has: full generic HTTP caching, not "did this one URL's byte
content change."

**Verdict:** hand-roll. Pulling `reqwest-middleware` + `http-cache-semantics`
(+ optionally `cacache`) to replace ~20 lines of conditional-GET glue is
disproportionate — violates the project's boring/minimal-dependency tech
strategy, and the crate is pre-1.0 (alpha) besides. Revisit only if Grimoire
grows a second, genuinely generic HTTP-caching need elsewhere.

## 3. OCI HEAD-manifest semantics across registries

The current [`opencontainers/distribution-spec` (main)](https://github.com/opencontainers/distribution-spec/blob/main/spec.md)
has tightened the requirement over older drafts:

> "A `HEAD` request to an existing blob or manifest URL **MUST** return `200 OK`. A successful response **MUST** contain the digest of the uploaded blob or manifest in the header `Docker-Content-Digest`. A successful response **MUST** contain the size in bytes... in `Content-Length`."
>
> "Implementers note: Clients may encounter registries implementing earlier spec versions which did not require the `Docker-Content-Digest` header."

So spec-current registries **MUST** emit the digest on HEAD; the spec itself
flags that older/lagging implementations may not. In practice, this is the
exact mechanism `crane`, `skopeo inspect`, `regctl`, and oci-client's
`fetch_manifest_digest` (already used in-tree) all depend on, across every
registry checked:

| Registry | HEAD + `Docker-Content-Digest` | Notes |
|---|---|---|
| Docker Hub | Yes, well-established | canonical implementation the spec is modeled on |
| GHCR (ghcr.io) | Yes | no reported gaps |
| GitLab (Container Registry, .com & self-managed) | Yes | Go rewrite (`gitlab-org/container-registry`) is spec-conformant |
| Harbor | Yes | built on `distribution/distribution`, fully spec-conformant |
| AWS ECR | Yes | ECR shipped full OCI Image/Distribution **1.1** conformance, with a published [conformance test report](https://oci-conformance.s3.amazonaws.com/distribution-spec/ecr/push/report.html) ([AWS OSS blog](https://aws.amazon.com/blogs/opensource/diving-into-oci-image-and-distribution-1-1-support-in-amazon-ecr/)); one known gap is unrelated to HEAD (referrer-manifest push via `oras copy -r` returns 405 — [aws/containers-roadmap#2783](https://github.com/aws/containers-roadmap/issues/2783)) |

**Verdict:** HEAD-manifest digest comparison is reliable across all five
registries grim targets. Keep the existing soft-fallback behavior (treat a
missing/absent digest header as "can't shortcut, do the full fetch") to cover
the spec's own "older registries" caveat — worth confirming that fallback
exists in the current `fetch_manifest_digest` call site.

## 4. Rate limits — the decisive question

This is where the three registries diverge sharply, and where the *shape* of
the risk matters more than a flat request count:

| Registry | Does HEAD count against pull/rate quota? | Real quota | Practical risk at ~500 HEADs/launch |
|---|---|---|---|
| **Docker Hub** | **No** — official docs: *"A Docker pull includes both a version check and any download... Version checks do not count towards usage pricing... Using GET emulates a real pull and counts towards the limit. Using HEAD won't."* ([docs.docker.com/docker-hub/usage/pulls](https://docs.docker.com/docker-hub/usage/pulls/)) | Pull quota: 100/6h (anon), 200/6h (authenticated free tier) — but HEAD is exempt | Exempt from the *pull* quota, but subject to Docker's separate, **undocumented** overall/abuse throttle observed at ~2000-3000 req/min before 429s with `Retry-After: 120` ([field-tested writeup](https://www.augmentedmind.de/2024/12/15/docker-hub-rate-limit-head-request/)) — a burst of 500 is fine, sustained high-frequency polling across grim's user base is the actual exposure |
| **GHCR** | No published pull quota for public images at all (unlimited pulls stated in GitHub's docs/community threads) | No fixed number | No documented cap, but real-world reports show 429s under **concurrent/parallel** load (CI matrix jobs hammering ghcr.io simultaneously — [github/community#134682](https://github.com/orgs/community/discussions/134682), [NVIDIA/cuda-quantum#3979](https://github.com/NVIDIA/cuda-quantum/issues/3979)). The trigger is *concurrency*, not sequential count. |
| **GitLab** | HEAD manifest still needs a bearer token from `/jwt/auth` unless a still-valid cached token is reused | GitLab.com: an IP is blocked 15 min after 300 **failed** auth requests/min on Git+registry auth combined ([GitLab rate-limit docs](https://docs.gitlab.com/user/gitlab_com/)) | Successful auth isn't the documented limit, but requesting a **fresh JWT per HEAD** (rather than reusing one token per repo-scope for the whole revalidation pass) multiplies request count for no reason and edges toward the failed-auth threshold if any requests error |
| **Harbor** | Operator-configured (nginx/ingress), no fixed platform number | n/a | Depends on deployment; not grim's problem to solve generically |
| **AWS ECR** | Standard AWS API throttling, generally generous for read ops | Not published as a fixed "requests/min" | Low risk for a few hundred HEADs; account/region throttling is the ceiling, not a hard documented quota |

**Decisive finding:** for the two registries with real observed friction
(Docker Hub, GHCR), **the failure mode is concurrency/burst rate, not raw
per-launch count.** Firing 500 HEAD requests *serially with modest pacing* is
safe on every registry checked; firing 500 *in parallel* is what the GitHub
issues above actually report failing on. This reframes the mitigation:
**cap in-flight concurrency (e.g., a semaphore of 8-16), don't cap total
count.**

## 5. `git ls-remote` vs `--depth 1` clone

`git ls-remote <url> <ref>` is a single round trip to the ref-advertisement
endpoint (`info/refs?service=git-upload-pack` over smart HTTP, or the
equivalent SSH handshake) — it is literally the first step `git fetch`/`git
clone` perform internally before deciding what to transfer. It returns just
the SHA for the requested ref: negligible bytes (well under 1KB for a
single-ref query), no working tree, no pack negotiation.

A `--depth 1` clone pays for that same ref advertisement **and then
unconditionally negotiates and downloads a full packfile** for that commit —
i.e., the entire current index tree — every single time it's invoked,
regardless of whether content changed since the last check. It cannot skip
the transfer just because nothing moved; that's exactly the job `ls-remote`
does for free. (General git plumbing behavior — see
[git-ls-remote docs](https://git-scm.com/docs/git-ls-remote),
[git-clone docs](https://git-scm.com/docs/git-clone) on `--depth`.)

**Verdict:** `git ls-remote <url> HEAD` (or the tracked branch ref) is the
correct, cheap "did the tip move" primitive. Compare the returned SHA against
the last-fetched SHA; only pay for a real fetch/clone when it differs.

**Failure mode:** a SHA change doesn't guarantee the specific file(s) grim
cares about changed (an unrelated commit could bump the tip) — a harmless
false-positive (unnecessary rebuild), never a false-negative, under the
standard assumption of fast-forward-only pushes to the tracked ref. The only
real race is the ordinary TOCTOU between `ls-remote` and the subsequent
fetch (ref moves in between) — not special to this scheme, same as any
polling-based check.

## 6. Recommendation — per source kind

| Source kind | Cheapest correct revalidation | Failure mode | TTL safety |
|---|---|---|---|
| **HTTP index** (GitHub Pages `all.json`) | Conditional GET: send cached `ETag` via `If-None-Match` (fall back to `If-Modified-Since` if only `Last-Modified` present); on `304`, reuse cached body and bump the freshness clock. On any response missing both validators, fall back to today's unconditional GET. | A CDN/edge layer that strips validators silently degrades to current unconditional-GET behavior — safe, not a regression. | **Safe at 5 min.** Conditional GET is near-free (tiny request, `304` has no body) and GitHub Pages has no meaningful rate limit for one low-volume client. |
| **Git index** | `git ls-remote <url> <tracked-ref>`; compare SHA to last-known; only clone/fetch on a diff. | Harmless false-positive rebuild on unrelated-commit tip moves; ordinary TOCTOU race with the follow-up fetch. | **Safe at 5 min.** One tiny ref-advertisement round trip; no meaningful cost even polled every launch. |
| **OCI `_catalog` walk** | Reuse the existing `HEAD` + `Docker-Content-Digest` per-repo/tag digest check (already implemented for tag resolution); compare against last-cached digest per package. | Spec's own caveat: older/non-conformant registries may omit the digest header — must fall back to a full manifest GET when absent (verify this fallback exists at the current `fetch_manifest_digest` call site). | **This is the one to flag as unsafe at 5 min if done naively.** The per-check cost is fine (HEAD is cheap and exempt from Docker Hub's pull quota), but a *browse catalog* means N packages × a HEAD each, repeated every 5 minutes across every grim install hitting shared public registries. Recommend: (a) keep concurrency bounded (semaphore ~8-16, not 500-wide fan-out) regardless of TTL — this is what actually trips Docker Hub's undocumented overall throttle and GHCR's observed 429s; (b) either keep the OCI catalog TTL closer to the current 1h, or add jitter across installs so revalidation doesn't synchronize; (c) for GitLab specifically, reuse one bearer token per registry+scope for the whole revalidation pass rather than re-authenticating per package. |

**Top-level recommendation:** adopt conditional-GET/HEAD-digest revalidation
for all three source kinds — it is strictly cheaper and more correct than
wholesale rebuild-on-TTL-expiry, and every mechanism involved (ETag/304 on
GitHub Pages, HEAD+digest on OCI, `ls-remote` on git) is well-supported,
spec-backed, or officially documented as free of quota cost. Hand-roll the
~20 lines of reqwest conditional-GET logic rather than adding
`http-cache-reqwest` (alpha, solves a bigger problem than needed). The one
real hazard is **concurrency, not TTL**, on the OCI leg against Docker Hub
and GHCR — bound in-flight HEAD requests and consider a longer/jittered TTL
specifically for the OCI catalog walk if it fans out across many packages.
