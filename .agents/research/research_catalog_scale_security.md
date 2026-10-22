# Research: Catalog sync security for large indexes

## Metadata

**Date:** 2026-10-17
**Domain:** security
**Triggered by:** /hex-architect catalog at scale (axis: security & compliance)
**Expires:** 2027-04-17

Scope: fetching, caching and syncing a static JSON catalog (flat `all.json` today; `root.json` -> shard -> record
tree later) over HTTP(S) from GitHub/GitLab Pages-style hosts, in `grim` (reqwest 0.13, tokio). Layout migrations are
in `research_catalog_sync_migrations.md` (not repeated). Tags: **[spec]** normative, **[doc]**, **[src]**, **[3p]** weak.
`[>18mo]` = older than 2025-04-17, re-verify if the claim is perishable. No network from the shell: nothing here was
measured on a live Pages host.

**Ground truth.** Commit 4650ad83 has none of the transport hardening the brief assumes: reqwest is
`default-features=false, features=["rustls"]` (`Cargo.toml:58`), `fetch_http` reads an unbounded `.bytes()` under one
30 s total timeout (`src/catalog/index_source.rs:34,585-605`), and a failed refresh returns `Err`. Gzip, the 128 MiB
cap and stale-serve are the unmerged `plan_index_scale_pre500.md` D4/D5. Trust boundary: `arch-threat-model.md`
(T1-T5, N1-N5, I1-I6) and `adr_artifact_trust_model.md` (decision 3: integrity not authenticity; decision 4 table).

## Direct answer

1. **Decompression:** reqwest bounds nothing. Count *decoded* bytes in a `chunk()` loop (the plan's `read_capped` is
   right). Advertise **gzip only** and call `no_brotli()/no_zstd()/no_deflate()` so feature unification cannot widen it.
   gzip is the only encoding with a proven ratio ceiling (1032:1); the decoded cap is the control either way.
2. **Digests:** stream-hash under the cap, compare to the parent's digest, then parse, then persist; make a new root
   current only when every file reachable from it is present and verified. This proves internal consistency (no
   mix-and-match, no CDN skew, local tamper evidence). It proves **no authenticity** while `root.json` is unsigned and
   same-origin: the origin can mint a self-consistent tree. That is decision 3, restated.
3. **Stale cache:** serve it, show its age, never refuse on age. Treat `generation` as a **consistency** counter on an
   unsigned root: older root = keep the cache and warn; never wedge on a huge one (fast-forward). Real freeze/rollback
   protection arrives only with signing.
4. **Signing reservation:** reserve a **detached** signature sibling over the exact `root.json` bytes plus
   `generation`/`generated_at`/`schema` fields. It costs nothing now and needs no layout change. The verification key
   must be pinned in client config, never declared by the index.
5. **Conditional GET / redirects:** ETag is an optimization, never a correctness input: no source shows content-derived
   ETags on either host. Content-addressed files make cross-origin redirects harmless (the digest decides); mutable
   files get a custom policy (<=5 hops, never https->http).
6. **Probe:** hostile bytes at `<dir>/c/root.json` are no worse than hostile `all.json` (same writer). Hold the probe to
   the same caps. Cache only 404/410 as "miss", with a short TTL; 5xx, timeouts and 200-garbage are "unknown".
   A cached miss becomes a downgrade vector once signing exists.

**Threat-model gap to fix in the ADR:** no T-id names the **index host/curator** (closest: T1, compromised curator) or
the **on-path attacker** (TLS; but `classify_index` accepts any `http://` locator, `registry_resolve.rs:60`, and
`insecure=true` is rejected on index entries). Add both as explicit rows. T3 is live: a cloned repo's committed
`[[registries]] index = ...` can point grim at an attacker host.

## Trends

- **Trending:** sidecar attestations over full TUF for package indexes (PyPI PEP 740 GA, Sigstore-backed;
  npm per-version `dist.signatures`); monotonic number + validity window for static repositories (RPKI manifests, APT
  `Date`/`Valid-Until`); decoded-size caps added after the 2026 auto-decode CVEs.
- **Established:** parent-listed hashes with consistent snapshots (TUF, APT by-hash, PEP 458 design); conditional GET on
  static trees (Cargo sparse index); HTTPS as the only transport protection (Cargo sparse index is unsigned).
- **Emerging:** TUF-on-CI (online signing from CI via Sigstore/KMS; used for the Sigstore root and GitHub attestations);
  zstd as an HTTP content coding (RFC 9659, 2024; no static host documented serving it); cosign `--bundle` as a
  one-file detached signature.
- **Declining/stalled:** full TUF deployment on PyPI (PEP 458 accepted 2019, implementation still "ongoing" in the last
  source found, 2022) [>18mo]; Cargo index signing ("may be challenging", 2018; no 2026 status found) [>18mo]; `deflate` as a content coding
  (zlib-vs-raw ambiguity) [3p]; Rust TUF/Sigstore libraries are not 1.0 (below).

## Key findings

**F1. reqwest's auto-decode is unbounded and hides the length.** `gzip(true)` removes `Content-Encoding` and
`Content-Length`; `Response::content_length()` is unknown for gzipped bodies; `ClientBuilder` offers on/off toggles and
no size option. The 0.12.9 decoder wires `async_compression::GzipDecoder` through `FramedRead` with no counting; 0.13's
`gzip` feature maps to `tower-http` `decompression-gzip`. The same defect class hit Elixir `Req` (CVE-2026-49755,
CWE-409: "a sub-megabyte response can expand to multiple gigabytes") and `httplib2` (CVE-2026-59939), both 2026.
Chunk counting needs no `stream` feature (`chunk()` does not; `bytes_stream()` does). Pre-check
`content_length()` when `Some` (identity responses) and reject early; with gzip count while reading. Overshoot is one
decoded chunk (inferred from the decoder shape). The cap bounds bytes, not RSS: a synthetic benchmark measured RSS at
~5.6x the raw document size, so a hostile 127 MiB document costs ~700 MiB; accept and write it down. Maps to the trust model's in-scope
row "resource exhaustion from hostile or corrupt registry data" (decision 4); attacker T1-class curator, or T3 via a
repo-declared index. Sources: docs.rs ClientBuilder/Response; reqwest `decoder.rs@v0.12.9`; docs.rs reqwest features;
osv.dev CVE-2026-49755; vulert httplib2.

**F2. Encoding choice: gzip only.** DEFLATE's ceiling is 1032:1 (zlib tech notes; ~1030:1 in practice), so 128 MiB of
output needs >=~130 KB of input (matches the plan's gzip-bomb test). Brotli: RFC 7932 s12 warns of resource exhaustion
and tells decoders to enforce "a hard limit"; no ratio ceiling. I found no primary ratio figure for brotli or zstd
(one search summary quoted 386516:1 for brotli with no identifiable source: **unverified, do not cite**). zstd: RFC 9659
caps Window_Size at 8 MB in HTTP, which bounds decoder *memory*, not output. Hosts: GitLab Pages precompresses only
htm/html/xml/txt/js/css/svg, no json, and does not compress on demand (research_catalog_sync_migrations s.C); GitHub
Pages brotli has no source newer than 2020 [>18mo]; GitHub Pages gzip seen on HTML only [3p]. br/zstd would add
dependencies and attack surface for no demonstrated host benefit. `flate2` defaults to pure-Rust `miniz_oxide`.
Mapping: same row as F1. Sources: zlib.net/zlib_tech.html; rfc-editor RFC 7932, RFC 9659; flate2 docs.

**F3. Digest chain: integrity yes, authenticity no.** TUF gets mix-and-match protection from the parent listing each
child's hash (spec 1.0.37 s5.5.2, s5.6.2); APT likewise ("signed Release file and hashes of other files", Klode 2017)
[>18mo]. Both anchor the chain in a *signed* root. Ours is the same chain without the anchor: it defeats partial deploys,
stale-edge skew, truncation, and (as evidence, I5) local edits under N2. It does not defeat the origin, whose compromise
is out of scope by decision 3 until signatures exist. Cargo's sparse index likewise relies on HTTPS alone (RFC 2789
[>18mo]). Catalog data never arms anything (I1 untouched); T2 is answered at install by the lock's manifest digest, so a
catalog lie changes *what the user sees*, not *what bytes install*. Weak spot: record text (`description`, `summary`,
`keywords`) reaches the terminal and MCP `grim_search`, i.e. T4 content; a digest says nothing about its safety.
`tui/render.rs:156` sanitizes control chars for member labels; I did not verify the same for `grim search` text output.

**F4. Verify-before-use, concretely.** (a) Hash while reading under the cap; compare to the parent's digest; only then
`serde_json`; only then write to the cache via temp+rename. (b) Digests: lowercase hex, exactly 64, algorithm named in the
path (`sha256`), unknown algorithm = skip layout. (c) URLs are built from a template plus the digest, never taken from
JSON, so no field can name an origin. (d) Bound structure: shard count, entries per shard, bytes per file; carry `size`
per child in the parent (TUF s4.4: absent length means "the client MUST use a custom download limit"). (e) Shard keyed by
`sha256(name)` prefix: reject entries that do not hash into their bucket, and derive the pointer's name/namespace from
structure, not the declared field (`walk_metadata` already does this for the git arm). (f) Complete-before-current:
download and verify the whole delta into a staging set, then flip one "current root" pointer; any 404/mismatch aborts and
keeps the old tree. RPKI does the same: hash mismatch = failed fetch, "continue to use cached versions"
(RFC 9286 s6.5-6.6) [>18mo]. Maps to I2 (identity is content, re-checked at use) and I5.

**F5. Freeze and rollback, unsigned vs signed.** TUF names rollback, indefinite freeze, mix-and-match and
**fast-forward** (inflating versions to block future updates); clients "MUST NOT replace a metadata file with a version
number less than the one currently trusted" and abort on a lower timestamp version. APT refuses a `Date` that goes
backwards and uses `Valid-Until` for freeze (Klode 2017) [>18mo]. RPKI: `manifestNumber` strictly increases, a lower one
falls back to cache, a stale `nextUpdate` is a failed fetch, a future `thisUpdate` too (RFC 9286 s4.2.1, s6.3)
[>18mo]. Consequence for an **unsigned** root: a monotonic check that *blocks* is a self-inflicted DoS, because the origin
can publish `generation = 2^63` once and every client refuses all later roots. So on an unsigned root: record the
highest seen, older = keep cache + warn, equal generation with a different digest = log inconsistency, and any
`--refresh` resets the record. Expiry is worse unsigned: a lapsed deploy degrades the whole fleet (APT clients "refuse to
use Release files that have expired", Launchpad docs), so reserve `expires`, do not enforce it. Maps to: not T2; I3
(degrade, never block); impact is stale `deprecated`/`replaced_by` data, bounded by the lock-pinned install path.

**F6. Stale cache policy.** The plan's D5 (serve cache on failed refresh, `refresh_error` in memory) is right. Add:
age is always shown (the existing `built_at` warning); a soft nudge past ~7 days (Launchpad's Backports example uses 14d
validity / 7d refresh, one data point); no hard cutoff because `grim add <ref>` does not depend on the catalog (I3).
Homebrew falls back to its cached copy on download failure and uses a 7-day stale default (prior research s.B;
https://raw.githubusercontent.com/Homebrew/brew/HEAD/Library/Homebrew/api.rb).

**F7. Reserving signing.** Precedents: TUF `{signed, signatures[{keyid,sig}]}` envelope (needs canonical JSON); npm
per-version `dist.signatures` over `name@version:integrity`, keys at `/-/npm/v1/keys` with `expires`; Homebrew `.jws.json`
(pinned key, "Potential MITM attempt detected"); cosign `sign-blob --bundle` (one detached file); minisign detached
`.minisig`; PEP 740 attestations as sidecars. **Pick detached:** sign the *exact decoded bytes* of `root.json`; no
canonicalization, no schema change, and old clients never request the sibling, so it is 100% additive (Principle 9).
What to reserve now: `schema`, `generation`, `generated_at` in the root; `sha256` in paths (algorithm agility); a note
that `<root>.sig`/`.sigstore.json` is a reserved sibling name. Key pinning must come from client config or a built-in
default, never from the index (a same-origin key is TOFU = unsigned; the repo's own memory notes index origin never
matches the forge host; project memory note `gitlab-pages-separate-domain`). Decision 3's "when signatures arrive" already says: gate at fetch time, uniform across kinds,
policy-controlled. Libraries: `sigstore-rs` README says "will not be considered stable until the 1.0 release" and
verification of attestations is unsupported; `tough` (AWS) is live (0.22.0, 2026-04 per a third-party aggregator [3p]);
`minisign-verify` is zero-dependency. Take no dependency now. TUF-on-CI is the emerging path if full TUF is ever wanted.

**F8. Conditional GET: do not lean on it.** One third-party measurement on GitHub Pages: ETag = mtime+size in hex,
deploy-stamped `Last-Modified`, `max-age=600`, weak ETag under gzip; the identical-bytes-redeploy case was *not*
observed on Pages. GitLab Pages: `max-age=600` (issue 711), validators undocumented; a GitLab 18.4+ regression adds an
empty `Content-Encoding` on proxied artifacts (issue 1167), a reminder that host encoding headers are not trustworthy
inputs. RFC 9110 lists 404 among heuristically cacheable codes; RFC 9111 s4.2.2 suggests ~10% of `Last-Modified` age as
heuristic freshness. For the tree, immutable files need no validator (digest-named, cacheable forever) and `root.json`
is ~20 KB, so a full refetch costs less than designing around ETag survival. For flat `all.json`, use `If-None-Match`
when present, fall back to unconditional, treat 304 as "keep what you have" only (never as authenticity); `error_for_status`
does not error on 3xx. Re-measure with the `curl` spot-check already recommended in `research_catalog_revalidation_http.md`.

**F9. Redirects.** reqwest 0.13.5: `Policy::default()` = `limited(10)`; on a host, port **or scheme** change it strips only
`Authorization`, `Cookie`, `Cookie2`, `Proxy-Authorization`, `WWW-Authenticate` (`redirect.rs`); there is **no https->http
block** (a scheme change only strips headers; `https_only(true)` is the blunt tool); `Referer` is on by default but not
sent on https->http. Custom headers (e.g. a future `PRIVATE-TOKEN`) are *not* stripped: the trust model's in-scope row "a
credential reaching the wrong host" applies the day private-index auth lands. GitHub Pages "Enforce HTTPS" redirects
http->https; apex<->www redirects are documented; I found no source for `github.io` -> custom-domain redirects. A repo-declared
(T3) host can redirect a CLI run in CI to an internal address: blind GET, no body echoed. Policy: `Policy::custom`,
<=5 hops, refuse https->http, mutable files (`all.json`, `root.json`) may follow cross-origin only to https; content-addressed
files may follow anywhere https (digest decides), which makes CDN offload free. Consider `referer(false)`.

**F10. Probe discovery.** Writer of `<dir>/c/root.json` = writer of `all.json`; no new principal. Path tenancy
(`user.github.io/repo/`) keeps the sibling inside the configured directory. Risks: (a) derive the sibling with
`url::Url`, not string slicing (`stats_url` uses `rsplit_once('/')` and mishandles `?query`/`#frag`); (b) soft-404 hosts
returning 200 + HTML must parse-fail into "miss", not an error; (c) outcome classes: 404/410 = miss (cache <= catalog
TTL, key includes grim version so an upgrade re-probes); 5xx/timeout/garbage = unknown (do not cache; keep any tree
cache, else `all.json`); (d) intermediaries cache 404s too, so expect up to ~10 min lag after an operator enables the
layout; (e) the probe gets its own short timeout (<=10 s) so a slow host cannot delay the `all.json` fallback;
(f) fallback amplification: forcing `all.json` costs ~7 MB vs ~30 KB at 20,000 entries, so rate-limit fallback to the catalog TTL;
(g) once a locator has served a **signed** root, a later 404 must not silently downgrade to unsigned `all.json`.

## Sources (URL - what it covers)

- https://docs.rs/reqwest/latest/reqwest/struct.ClientBuilder.html - gzip/brotli/zstd/deflate toggles, header removal, no_*, redirect, timeouts (0.13.5)
- https://docs.rs/reqwest/latest/reqwest/struct.Response.html - content_length unknown when decoded; chunk; bytes_stream needs `stream`
- https://docs.rs/crate/reqwest/latest/features - gzip -> tower-http decompression-gzip; none default
- https://raw.githubusercontent.com/seanmonstar/reqwest/v0.12.9/src/async_impl/decoder.rs - no output limit; async-compression; Content-Length removed
- https://raw.githubusercontent.com/seanmonstar/reqwest/master/src/redirect.rs - default 10 hops, stripped headers, no downgrade block
- https://osv.dev/vulnerability/CVE-2026-49755 - Elixir Req decompression bomb (2026-06-08); https://vulert.com/vuln-db/httplib2-decompression-bomb-denial-of-service-via-unbounded-gzip-deflate-response-handling
- https://www.zlib.net/zlib_tech.html - 1032:1 theoretical, ~1030:1 practical [>18mo, stable]
- https://www.rfc-editor.org/rfc/rfc7932.html (s12) - brotli resource-exhaustion guidance [>18mo, normative]
- https://www.rfc-editor.org/rfc/rfc9659.html - zstd Window_Size <= 8 MB in HTTP (2024)
- https://specification.website/spec/performance/compression/ - deflate "avoid" [3p]
- https://theupdateframework.github.io/specification/latest/ - attacks, rollback/version rules, root layout, hashes, lengths (v1.0.37, 2026-10-08)
- https://peps.python.org/pep-0458/ - roles, expiry, consistent snapshots; status Accepted; https://discuss.python.org/t/pep-458-current-status-and-next-steps-feedback-requested/17211 - implementation ongoing [>18mo]
- https://blog.sigstore.dev/pypi-attestations-ga/ - PEP 740 attestations GA, Sigstore-backed
- https://docs.npmjs.com/about-registry-signatures - dist.signatures, signed message, key endpoint
- https://rust-lang.github.io/rfcs/2789-sparse-index.html - HTTPS only, signing "may be challenging", ETag/Last-Modified, cache-buster [>18mo]
- https://blog.jak-linux.org/2017/08/17/why-tuf-does-not-shine-for-apt-repositories/ - APT Valid-Until, Date monotonic, hash chain [>18mo]
- https://ubuntu.com/docs/launchpad/developer/explanation/core-components-and-set-up/publisher/valid-until/ - expired Release files refused
- https://www.rfc-editor.org/rfc/rfc9286.html - RPKI manifestNumber, nextUpdate, hash mismatch keeps cache [>18mo, normative]
- https://www.rfc-editor.org/rfc/rfc9111.html (s4.2.2); RFC 9110 s15.1 heuristically cacheable list via https://github.com/psf/cachecontrol/pull/405 [3p]
- https://dev.to/jangwook_kim_e31e7291ad98/your-last-deploy-reset-every-cache-validator-on-the-site-4dd3 - GitHub Pages ETag/Last-Modified/max-age [3p]
- https://gitlab.com/gitlab-org/gitlab-pages/-/issues/711 and /-/issues/1167 - GitLab Pages max-age=600; empty Content-Encoding regression
- https://docs.github.com/en/pages/getting-started-with-github-pages/securing-your-github-pages-site-with-https ; .../about-custom-domains-and-github-pages - HTTPS enforcement redirect; apex/www redirects
- https://docs.sigstore.dev/cosign/signing/signing_with_blobs/ - sign-blob --bundle; https://github.com/sigstore/sigstore-rs - experimental, pre-1.0
- https://github.com/awslabs/tough ; https://github.com/theupdateframework/tuf-on-ci - Rust TUF client; CI-signed TUF (Sigstore root, GitHub attestations)
- https://docs.rs/minisign-verify - zero-dependency Ed25519 verifier
- Repo: `src/catalog/index_source.rs`, `Cargo.toml:58`, `src/config/registry_resolve.rs:52-64`, `src/config/project_config.rs:1367`, `.agents/plans/plan_index_scale_pre500.md` D4/D5, `.agents/discussions/index-scale-rollout.md`, `research_catalog_sync_migrations.md` s.C, `research_catalog_revalidation_http.md`

## Recommendation

| Item | Status | Rationale |
|---|---|---|
| Decoded-byte cap on every body (all.json/stats 128 MiB; root 1 MiB; shard 512 KiB; record 64 KiB = >=25x observed) | **Required now** | F1; trust-model row "resource exhaustion"; T1-class curator, T3 repo-declared index. Cap bounds bytes not RSS (~5.6x) |
| `content_length()` pre-check plus chunk counting; no `.bytes()` anywhere | **Required now** | Identity bodies reject early; gzip counts while reading |
| Advertise gzip only; `no_brotli/no_zstd/no_deflate` on the index client | **Required now** | F2; only encoding with a proven ratio ceiling; no host evidence for br/zstd on JSON |
| br / zstd | **Non-goal** | New deps and bomb surface, no measured gain; revisit if a target host is shown to serve them on `.json` |
| Compressed-size cap / manual decode with ratio check | **Non-goal** | Decoded cap plus overall timeout already bound memory and time |
| Stream-hash, verify, then parse, then persist (temp+rename) | **Required now** (ships with the tree) | F4; I2/I5; poisoned or truncated files never reach the cache |
| Strict digest grammar; URLs from template only; bucket-consistency; structure-derived names; caps on counts | **Required now** (ships with the tree) | F4; stops a hostile root naming origins, oversized fan-out or cross-bucket injection |
| Complete-before-current (stage, verify all, flip one pointer; abort keeps old tree) | **Required now** (ships with the tree) | F4; mix-and-match and half-sync states |
| `size` per child in root/shard | **Reserve** (put in the v1 shape) | TUF s4.4 length rule; additive but best shipped with the first client |
| Authenticity of an unsigned same-origin root | **Non-goal** (record it) | Decision 3; origin compromise out of scope until signatures; say "integrity, not authenticity" in the ADR |
| `generation` (u64), `generated_at`, `schema` in root | **Reserve** (put in the v1 shape) | F5; consistency now, rollback protection once signed |
| Rollback *enforcement* (refuse older/huge generation) on unsigned root | **Non-goal** | Fast-forward wedge; behaviour is keep-cache + warn; `--refresh` resets |
| `expires` field | **Reserve** (name only, never enforced unsigned) | Unsigned expiry self-DoSes the fleet on a lapsed deploy |
| Stale-serve on failed refresh with age shown; soft nudge ~7d | **Required now** | F6; I3; hard cutoff is a **non-goal** (`grim add <ref>` is catalog-independent) |
| Detached signature sibling over exact root bytes (`.sig`/`.sigstore.json`) | **Reserve** (name only) | F7; additive, no canonical JSON; do not ship verifier or dependency now |
| Verification key source | **Reserve** (client config or built-in only) | An index-declared key is TOFU = unsigned; policy hooks into decision 3 "when signatures arrive" |
| TUF roles, thresholds, revocation, transparency log | **Non-goal** | Exceeds the freeze-era scope; TUF-on-CI is the upgrade path if wanted |
| ETag/Last-Modified conditional GET | **Reserve** (ship on the Merkle trigger) | F8; unmeasured on both hosts; digest is the validator for immutable files; never an authenticity signal |
| Redirect policy: custom, <=5 hops, no https->http, mutable files cross-origin https only, immutable files any https | **Required now** (ships with the tree probe) | F9; T3 SSRF-lite, credential-leak row; pre500 plan excludes it, so schedule it with the probe |
| Strip custom auth headers on cross-origin redirect | **Reserve** | reqwest strips only five headers; matters when private-index auth lands |
| Probe: `Url`-based sibling, own <=10 s timeout, miss = 404/410 only, TTL <= 1 h, keyed by grim version | **Required now** (ships with the tree probe) | F10; avoids cache-poisoned downgrade and slow-host stalls |
| "Once signed, never silently fall back to unsigned" | **Reserve** | F10(g); needs a per-locator "seen signed root" bit in the cache schema now |
| ADR threat table: add index host/curator and on-path attacker rows | **Required now** (ADR text) | No T-id covers them; `http://` index locators are accepted today |
