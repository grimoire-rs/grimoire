# Research: Trust and integrity of an incremental index build that carries state between runs

## Metadata

**Date:** 2026-10-20
**Domain:** security & compliance
**Triggered by:** `/hex-architect` on `.agents/discussions/index-structure.md` (axis: security & compliance)
**Expires:** 2027-04-20

Scope: a static package index on GitHub Pages or GitLab Pages, rebuilt hourly by CI, that (1) restores blobs and a
per-package enrich checkpoint from the CI cache and falls back to its own published site, (2) skips work on an input
hash, (3) publishes a content-addressed tree (`c/root.json` -> plane index -> shard -> record/blob) beside a frozen
`all.json`, (4) retains 3 generations + 24 h. Companion: `research_catalog_scale_security.md` (client-side fetch:
decompression caps, digest-then-parse, TUF/APT notes). Not repeated here unless the answer changed.

Tags: **[doc]** vendor documentation, **[src]** source code read, **[spec]** normative text, **[adv]** advisory or
incident write-up, **[3p]** secondary source, **[probe]** run locally this session. `[>18mo]` = older than 2025-04-20.
No shell network access (curl was denied): **no response header of GitHub Pages or GitLab Pages was measured live**.

## Direct answer

1. **Verifying each restored blob against a digest does not neutralise cache poisoning on its own.** It neutralises
   only *content* substitution, and only when the digest comes from somewhere the cache writer cannot touch. Two
   things survive it: (a) the cache restore itself is an archive extraction that can overwrite files outside the
   cached paths, so a hostile writer gets code execution in the restoring job before any digest is checked
   ([Khan 2024](https://adnanthekhan.com/2024/05/06/the-monsters-in-your-build-cache-github-actions-cache-poisoning/)
   [>18mo]); (b) the *mapping* (input hash -> record/data) is not content-addressed, so a poisoned checkpoint
   entry that carries a valid-looking `input` is skipped forever. So the ADR must mandate **two rules, not one**:
   cache write authority never exceeds the reader's trust, and every digest is anchored in the last published root.
2. **Trusted manifest = the last published `c/root.json` (and the plane indexes and shards it names) fetched over
   HTTPS from the configured `site`, never the cache.** The cache supplies bytes only. A digest read out of the
   cache proves nothing (same writer controls both). The residual is transitive: a poisoned record that was already
   published is in the "trusted" root. Bound it with a forced deep re-enrich age and a `fresh` dispatch that ignores
   both cache and published state (Key finding 4).
3. **Platform defaults moved in 2026 and cut the most common path, but do not remove the need for the rule.** GitHub
   made cache tokens read-only for untrusted triggers on 2026-06-26 and shipped `cache-mode` on 2026-09-10. Both are
   github.com behaviour; a mandated explicit `cache-mode` and a single writer workflow cost nothing and survive a
   host change. GitLab separates protected and non-protected caches by key suffix, with open reports on self-hosted
   runners.
4. **Serving third-party README/changelog blobs as static files is safe only if the host's `Content-Type` is
   decided by an allow-listed extension.** GitLab Pages sniffs the first 512 bytes with Go's `DetectContentType`
   when the extension is unknown or absent, and `<p`, `<div`, `<h1` are HTML signatures there: an extensionless
   README that starts with `<p align="center">` is served as `text/html`. Neither host lets a project add
   `X-Content-Type-Options`. The ADR must forbid extensionless and scriptable-type blob paths and add a post-deploy
   header probe.
5. **The larger XSS exposure is the existing rendered pages, not the proposed blobs.** Measured: the Astro markdown
   processor the indexer uses passes `<script>`, `onerror=` and `javascript:` hrefs through, and the indexer's Astro
   config sets no sanitizer (Key finding 9). Raw blobs add new directly navigable URLs and a 3-generation retention
   tail (takedown latency), not a new class.
6. **`generation`: record it always; enforce it client-side only as a soft ratchet.** An unsigned same-origin root
   cannot stop an origin-level attacker; the ratchet defends against edge staleness and accidental older
   deploys. Hard failure on regression self-inflicts an outage when state is lost. Do not enforce an expiry.

## Trends

- **Cache poisoning went from research to production incidents in 2026.** Cline (disclosed 2026-02-09) and TanStack
  (2026-05-11) were both "low-trust workflow writes the default-branch cache, release workflow restores it".
- **Platforms responded at the cache service, not the workflow.** Read-only tokens for untrusted triggers
  (2026-06-26), `cache-mode` (2026-09-10), `actions/checkout` v7 refusing fork-PR checkout in `pull_request_target`.
- **Content-addressed mirrors converge on "keep old objects for a window".** Debian `Acquire-By-Hash`: current plus
  two or more previous. conda CEP 16: e.g. one week. The proposed 3 generations + 24 h sits inside that norm, but it
  is sized by time while Pages sizes by files (GitLab.com: 200,000 entries).
- **Freshness is signed everywhere it is enforced** (TUF, APT `Valid-Until`); the unsigned ecosystems (Cargo sparse,
  conda shards) rely on HTTP caching and do not enforce any expiry or monotonic version.

## Key findings

### 1. GitHub cache: scope, immutability, eviction (current)

- A run restores caches from its own branch or the default branch; a PR run also from the base branch; sibling and
  child branches are isolated. PR-created caches live in `refs/pull/N/merge` and are restorable only by re-runs of
  that PR. [doc] https://docs.github.com/en/actions/reference/workflows-and-actions/dependency-caching
- "You cannot change the contents of an existing cache"; replacement needs a new key. Only `push`/`schedule`-class
  triggers may write the default-branch scope; low-trust triggers are read-only. [doc] same page.
- Restore order: exact key, then key prefix, then `restore-keys` in order, most recently created match wins, then the
  whole sequence repeats on the default branch. [doc] same page. Consequence: the usual incremental pattern
  (`key: state-${{ run_id }}`, `restore-keys: state-`) hands the restore to **whichever writer created the newest
  matching entry**.
- Limits: 10 GB per repository default (billed above, configurable to 10 TB), entries unused for 7 days removed,
  LRU eviction when over the limit, 200 uploads and 1,500 downloads per minute per repo. [doc] same page;
  [changelog 2025-11-20](https://github.blog/changelog/2025-11-20-github-actions-cache-size-can-now-exceed-10-gb-per-repository/).
  The 10 GB is **shared repo-wide** with npm/`setup-*` caches, so an index state cache is evicted by unrelated
  growth; a cold start must be a tested path, not an error.
- "Anyone who can open a pull request against your repository can read the contents of caches in the base branch."
  Never put a token, credential or non-public data in the cache. [doc] same page.
- Scheduled workflows run only on the default branch and are **disabled after 60 days without repository
  activity** in public repos. [doc] https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#schedule
  An hourly index can silently freeze; this matters for any expiry enforcement (finding 7).

### 2. GitHub cache poisoning: documented attacks and the 2026 platform response

- **Khan, "Monsters in your build cache" (2024-05-06, updated 2025-01-23) [>18mo]:** the branch is the only
  boundary enforced server-side; key and version are client-computed strings; restore-keys prefix matching lets an
  attacker plant an entry under a predictable prefix; a lockfile-hash key can be predicted from open Dependabot PRs;
  the restore extracts the tar without checking contents against the configured `path`, so `package.json`, a
  Makefile or a reusable action's `index.js` can be overwritten. Needs code execution in any default-branch-scope
  workflow (Pwn Request, dependency, insider); a read-only `GITHUB_TOKEN` suffices. Advice: no caching in release
  builds; restore only intended paths; delete poisoned entries. The Jan 2025 update says post-job writes are no
  longer possible but in-build poisoning still works. https://adnanthekhan.com/2024/05/06/the-monsters-in-your-build-cache-github-actions-cache-poisoning/
- **Cacheract** (tool, listed Jan 2024 / repo Dec 2024 [>18mo]): automates entry replacement and persistence by
  hijacking the `actions/checkout` post step. https://adnanthekhan.com/projects/cacheract
- **Clinejection (2026-02-09):** a prompt-injected triage workflow (`issues` event, default-branch scope) flooded the
  cache past 10 GB to evict the nightly workflow's `node_modules` entry and wrote a poisoned one under the same
  `hashFiles('package-lock.json')` key; the publish job restored it and leaked publish tokens. Khan states that
  since 2025-11-20 eviction happens as soon as the cache exceeds 10 GB (before: every 24 h), so a flood works
  within one run. (The GitHub changelog confirms the LRU-on-overflow rule but not the timing.)
  https://adnanthekhan.com/posts/clinejection/
- **TanStack (2026-05-11) [adv]:** `pull_request_target` workflow checked out `refs/pull/N/merge`; a shared setup
  action's `actions/cache` post step wrote the attacker's pnpm store to the base cache scope (the job's
  `permissions:` did not govern the write); `release.yml` restored the same key and ran it with `id-token: write`.
  Fix advice: `pull_request`, key bump (`-v2`), audit (PR checkout + cache writer + shared key).
  https://safedep.io/tanstack-github-actions-cache-poisoning/ (a second write-up,
  https://www.landh.tech/blog/20260511-tanstack-supply-chain-compromise/, disagrees on whether the scope was "base
  repository" or "main"; both agree it was the scope the release job restores).
- **Platform response (primary):**
  - 2026-06-26: cache tokens are **read-only** when the event is untrusted (someone other than a collaborator can
    trigger it) *and* the cache scope is the default-branch SHA: `pull_request_target`, `issue_comment`,
    fork-`workflow_run`. `push`, `schedule`, `workflow_dispatch`, `repository_dispatch`, `delete`,
    `registry_package`, `page_build`, and non-default scopes (`pull_request`, `release`) stay read-write. A denied
    write is a warning, not a failure. https://github.blog/changelog/2026-06-26-read-only-actions-cache-for-untrusted-triggers/
  - 2026-09-10: `cache-mode` = `read` | `write` | `write-only` | `none`, workflow or job level, enforced by scoped
    tokens, reusable workflows cannot exceed their caller; an explicit `write` on a low-trust trigger overrides the
    read-only default and raises a warning annotation. https://github.blog/changelog/2026-09-10-control-github-actions-cache-access-with-cache-mode
    and https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax#cache-mode
  - `actions/checkout` v7.0.0 "Block checking out fork PR for `pull_request_target` and `workflow_run`"; override is
    the input `allow-unsafe-pr-checkout` (default `false`). https://github.com/actions/checkout/blob/main/CHANGELOG.md
    and https://github.com/actions/checkout/blob/main/README.md
  - **[3p, unverified]** a June 2026 write-up reports public repos without an event policy will have
    `pull_request_target` disabled from **2026-11-02** (evaluate mode now) and that the checkout block was
    backported on 2026-07-20 to supported majors for floating tags. I found no GitHub changelog entry.
    https://dev.to/jessehouwing/keep-the-creepy-crawlies-out-of-you-workflows-securing-github-actions-in-2026-2np0
- GitHub's own hardening page says `pull_request_target`/`workflow_run` workflows "share the same cache of the main
  branch with other privileged workflow triggers" and prefers `workflow_run` for privilege separation; it gives no
  cache-specific mitigation. [doc] https://docs.github.com/en/actions/reference/security/secure-use

**Observed in the indexer templates** (`grimoire-indexer/templates/ci/`, read this session, not a claim about any
deployed index):

- `github-validate.yml` is `pull_request_target`, checks out `pull_request.head.sha` into `pr-tree/` (as data), runs
  `npm ci --ignore-scripts` from the base tree, and declares `actions/setup-node` with `cache: npm`.
  `github-pages.yml` (build + deploy jobs, `pages: write`, `id-token: write`) also uses `cache: npm`. Same lockfile
  hash, same default-branch scope: **the Cline/TanStack shape** (low-trust writer, privileged reader, shared key).
  Platform defaults make the validate job's save a no-op on github.com now; a GitHub Enterprise Server host or a
  future default change reopens it. `npm ci` re-checks tarballs against lockfile `integrity`, which is the right
  pattern (digest from a trusted manifest), but the tar-extraction primitive from finding 2 is not covered by it.
- The validate workflow's checkout is pinned at `actions/checkout` v7.0.1 with `ref: ...head.sha`. Given v7's block
  on fork-PR checkout in `pull_request_target`, **the contribution gate may need `allow-unsafe-pr-checkout: true`
  or fail for fork PRs**. I did not run it. Treat as a verify-now item for the template owner; if the 2026-11-02
  auto-disable is real, a required `validate` context that never reports blocks every PR (the template's own
  warning).

### 3. GitLab CI cache

- Caches are per project. Protected and non-protected refs **do not share caches by default**: keys get a
  `-protected` / `-non_protected` suffix by the ref's (or the tag's) protection state. The docs call the separation a
  security feature and say to disable it only if every Developer is highly trusted. [doc] https://docs.gitlab.com/ci/caching/
- `cache:unprotect: true` lets users without protected-branch access "read and write to cache keys used by protected
  branches". [doc] https://docs.gitlab.com/ci/yaml/#cacheunprotect
- `fallback_keys` (up to five) and a global `CACHE_FALLBACK_KEY` are tried after `cache:key`; the documented pattern
  is a default-branch key. All caches of a job go in one `cache.zip`; jobs with the same key and different paths
  overwrite each other; caches restore before artifacts and can overwrite artifact paths; "Caching is an
  optimization, but it isn't guaranteed to always work". [doc] https://docs.gitlab.com/ci/caching/
- Runner-local cache (Docker volumes or the runner's home) vs distributed (S3; GitLab.com instance runners). [doc]
  same page.
- **Fork MR pipelines** run in the **fork** project, with the fork's config, resources and variables, unless a parent
  member chooses "run in the parent project" (parent runners and variables, fork's CI config). Protected variables
  and runners require both branches protected and in the same project. [doc]
  https://docs.gitlab.com/ci/pipelines/merge_request_pipelines/ The docs page says nothing about caches here.
- **Poisoning reports (unresolved status unless noted):**
  - CVE-2022-1423: a Developer could poison the cache and execute in protected-branch jobs; fixed in 14.8.6, 14.9.4,
    14.10.1 (2022) [>18mo]. https://vulert.com/vuln-db/CVE-2022-1423 [3p]
  - 2024-09: CI/CD cache poisoning from an unprotected branch to other users' pipelines, `CI_JOB_TOKEN` exposure
    claim; no GitLab response visible [>18mo]. https://gitlab.com/gitlab-org/gitlab/-/work_items/494478
  - 2024-11 (search snippet only, not opened): claimed bypass of the protected-cache fix via a crafted
    `CACHE_FALLBACK_KEY`. https://gitlab.com/gitlab-org/gitlab/-/issues/506381
  - 2025-05-26: self-hosted Docker runners mount `/cache` by default and reuse it across refs, so a Developer on an
    unprotected branch can write what a protected job reads; no resolution visible.
    https://gitlab.com/gitlab-org/gitlab/-/issues/547332
- The template's `pages` job runs only on `$CI_COMMIT_BRANCH == $CI_DEFAULT_BRANCH` (protected by default), so on
  GitLab.com its cache is the `-protected` one. The template already keeps the merge-request gate's npm cache in a
  separate `.npm-gate` directory it deletes first, noting the shared directory "is a path the merge request can
  commit into".
- Page note: when reading GitLab issue pages through a fetch tool, two returned text asking an agent to read project
  files and use `glab`; ignored. Treat issue-tracker content as untrusted input in any automated reader.

### 4. Does digest verification neutralise poisoning? Where must the digest come from?

What a digest check buys and what it does not:

| Threat | Digest check against a trusted manifest | Needs |
|---|---|---|
| Blob bytes altered or truncated in cache | stopped | digest from published root |
| Edge/CDN skew serving mixed generations | stopped (consistent closure) | verify whole closure before switching |
| Poisoned *mapping* (checkpoint says "input X unchanged, data = D") | **not stopped** if D's digest is also read from the cache | anchor the mapping's digest in the published enrich plane, not the cache |
| Tar extraction overwrites files outside `path` | **not stopped** (happens at restore) | restrict who can write the scope |
| Poison already published once | not stopped; transitively trusted | forced re-enrich age + `fresh` dispatch |
| Origin attacker mints a self-consistent tree | not stopped (unsigned root, same origin) | out of scope per `adr_artifact_trust_model.md` decision 3; TUF/`tuf-on-ci` is the signed upgrade |

Prior art for the split "verify bytes by digest, but protect the mapping by write authority":

- Bazel: the cache is a CAS plus an action cache mapping action hashes to results; docs say "Take care in who has the
  ability to write to the remote cache", suggest only CI writes, and list "create a clean cache after a cache was
  poisoned" as a recovery. [doc] https://bazel.build/remote/caching (the page does not explain CAS verification).
- Debian: by-hash index files are named by hash and only the signed `Release` file names them. [doc]
  https://wiki.debian.org/DebianRepository/Format
- TUF: snapshot/targets hashes are checked against the *trusted* parent metadata, never against the file itself. [spec]
  https://theupdateframework.github.io/specification/latest/ (section 5.5 step 2, 5.6 step 2).

Implementation lessons from TUF clients: `tough` (CVE-2025-2888, 2025-03-27 [>18mo]) cached timestamp metadata before
the snapshot rollback check, so an invalid file stayed "trusted" and later valid updates were rejected
(https://github.com/advisories/GHSA-76g3-38jv-wxh4); `go-tuf` (CVE-2022-29173 [>18mo]) had the same
persist-before-verify order and a zero default for the version check
(https://www.mend.io/vulnerability-database/CVE-2022-29173/ [3p]). Rule for the sync code: **persist the new root
only after the whole reachable closure is verified**, and compare against the previously accepted generation before
persisting, not after.

Optional escalation (not recommended by default): bind the checkpoint to the build workflow with an artifact
attestation or an HMAC kept in an environment-scoped secret, so a writer in another workflow cannot forge it.
Cost: more moving parts for a threat the single-writer rule already removes. [my inference]

### 5. Cache scoping rules the ADR can mandate (derived, mapped to the evidence above)

GitHub:

1. **One writer.** Only the hourly build workflow (events `schedule`, `workflow_dispatch`, `push` on the default
   branch) saves. Use split `actions/cache/restore` and `actions/cache/save`; save only after a successful publish.
2. **Every other workflow sets `cache-mode: none`** (or `read`), explicitly, including `setup-node` `cache: npm` in
   `pull_request_target` workflows. Do not depend on the 2026-06-26 default (github.com only).
3. **Key is not a boundary.** Namespace and version the key (`<index>-state-v1-...`) and bump the version after any
   incident. Prefer an **exact key bound to the published root digest** of the generation the cache was built for
   (no `restore-keys` prefix, so "newest matching wins" cannot select a hostile entry). [my inference]
4. **Data only.** The state cache holds blobs and the checkpoint under one dedicated directory; no `node_modules`,
   no npm cache, no tool binaries under the same key; no secrets (the cache is readable by anyone who can open a PR).
5. **Never share a key between a workflow that can run contributor-influenced code and one holding `pages: write`
   or `id-token: write`.** (Cline, TanStack.)
6. **Cold start is a tested path.** Cache eviction (7 days unused, repo-wide 10 GB LRU, 60-day schedule disable)
   must degrade to the published-site fallback; add a `fresh` dispatch input and run it in CI.

GitLab:

1. Keep "Use separate caches for protected branches" on; never `cache:unprotect: true`.
2. No `fallback_keys` / `CACHE_FALLBACK_KEY` that can resolve to a `-non_protected` entry from a protected job.
3. `pages` stays default-branch only. Do not widen MR pipelines to read the state key.
4. Self-managed Docker runners: the default shared `/cache` volume is reported to cross refs; use distributed S3
   cache, per-job ephemeral runners, or `disable_cache`/no volume for the index project. [3p reports, unresolved]
5. Same data-only, versioned-key, cold-start rules as GitHub.

### 6. Freshness and rollback: how others handle it, and what to enforce

- **TUF:** "Clients MUST NOT replace a metadata file with a version number less than" the trusted one (section 4.3);
  an equal timestamp version means "discard the new timestamp metadata and abort the update cycle", described as
  normal, not an error; expired metadata is refused; errors "should not leave clients in an unrecoverable state".
  [spec] https://theupdateframework.github.io/specification/latest/ (5.3-5.6)
- **APT:** `Date` older than the stored Release is ignored; `Valid-Until` is optional and "client behaviour on expired
  Release files is unspecified" in the format doc, but apt enforces it by default (`Acquire::Check-Valid-Until`
  true, `Max-ValidTime`, `Min-ValidTime`). [doc] https://wiki.debian.org/DebianRepository/Format,
  https://manpages.debian.org/trixie/apt/apt.conf.5.en.html
- **conda CEP 16:** index `repodata_shards.msgpack.zst` has `version: 1` and `info.created_at`; shards are
  `<sha256>.msgpack.zst`; index `Cache-Control: max-age` 60 s to 1 h; shards immutable; "keep old shards for a certain
  amount of time (e.g. 1 week)"; no rollback mechanism; the spec does not state that clients verify shard hashes.
  [spec, date not visible] https://conda.org/learn/ceps/cep-0016
- **Cargo sparse:** ETag/Last-Modified revalidation only; recommends CDN invalidation; no integrity or rollback
  mechanism for index entries documented; entries are append-only except `yanked`. [doc]
  https://doc.rust-lang.org/cargo/reference/registry-index.html (signing RFCs 2474/2789 proposed, status
  unconfirmed: https://rust-lang.github.io/rfcs/2789-sparse-index [>18mo])
- **Edge staleness is real and short:** GitLab Pages serves files with `Cache-Control: max-age=600` and a matching
  `Expires` [src] https://gitlab.com/gitlab-org/gitlab-pages/-/raw/master/internal/serving/disk/reader.go
  (`serveFile`); the discussion and `research_catalog_revalidation_http.md` record the same ~10 min on GitHub Pages
  (Fastly) [3p]. A client can legitimately see root g-1 after g, from another edge.
- **Debian's retention rule matches the proposal's shape:** with `Acquire-By-Hash: yes`, "the current version must and
  two or more previous versions of a file should be available". [doc] https://wiki.debian.org/DebianRepository/Format
  (the rationale post, https://www.chiark.greenend.org.uk/~cjwatson/blog/no-more-hash-sum-mismatch-errors.html
  [>18mo], could not be fetched; its search snippet states the same design goal).

What follows for `generation`:

- **Record always; enforce as a ratchet that fails soft.** The client keeps the highest accepted `generation` per
  `(index URL, epoch)`. A lower value is treated as stale: keep serving the held state, warn once, retry after the TTL,
  never exit non-zero and never overwrite the held root. This covers edge skew and an old workflow re-run (whether
  `actions/deploy-pages` refuses an older run is **not documented**: https://github.com/actions/deploy-pages).
- **It cannot defend against the origin.** Whoever deploys the site can set any number; only a signature helps.
- **Hard failure creates a self-inflicted outage** if the counter ever resets (lost cache, new repo, restored backup):
  every client that saw a higher number is wedged. So (a) derive the new `generation` from the published root plus
  one or from a monotonic CI counter, never from the cache alone; (b) add an `epoch`/`index_id` that an operator
  changes to declare an intentional reset, which clients treat as a new ratchet. [my inference]
- **Indexer side:** the indexer also reads a root over the same edge. If the root it fetches is lower than the
  high-water mark it recorded in the cache, it must use the higher and refuse to publish a number that collides with a
  different root digest.
- **Do not enforce `expires`.** Schedules are disabled after 60 days idle (finding 1); a hard expiry would turn that
  silent freeze into a simultaneous client outage. Reserve the field, surface it as a warning.

### 7. Size caps, many small files, path traversal

- gzip's format ceiling is about 1032:1 (about 1030:1 implemented). [doc, stable, >18mo] https://zlib.net/zlib_tech.html
  Count **decoded** bytes; cap per blob class from the parent entry's `size`; size is checked before hashing
  (OCI: size mismatch means "SHOULD NOT be trusted"). See `research_catalog_scale_security.md` for the reqwest
  details; unchanged.
- **Digest grammar.** OCI allows `algorithm:encoded` with `algorithm` = `[a-z0-9]+([+._-][a-z0-9]+)*` and `encoded` =
  `[a-zA-Z0-9=_-]+`, registers `sha256`, `sha512`, `blake3`, and says implementations SHOULD pass unrecognised
  algorithms. [spec] https://github.com/opencontainers/image-spec/blob/main/descriptor.md **Do not reuse that
  grammar for index digests.** Validate `^sha256:[0-9a-f]{64}$` (lowercase only; mixed case aliases on
  case-insensitive filesystems) before building any path or URL, and derive the path from the *validated* string.
  Precedent: Ollama before 0.1.34 did not validate the manifest digest format, enabling `../` writes and RCE
  (CVE-2024-37032, 2024 [>18mo]) https://www.rapid7.com/db/modules/exploit/linux/http/ollama_rce_cve_2024_37032/ [3p].
- **File-count ceilings are a compliance constraint of the host.** GitLab.com: 200,000 file entries per Pages site
  (directories and symlinks included). [doc] https://docs.gitlab.com/administration/instance_limits/ ; site size 1 GB
  https://docs.gitlab.com/user/gitlab_com/ . GitHub Pages: published site 1 GB, deploy timeout 10 min, no file-count
  limit stated. [doc] https://docs.github.com/en/pages/getting-started-with-github-pages/github-pages-limits
  My estimate, not measured: at roughly 5-6 published files per package (page, record, up to four blobs) the GitLab
  entry limit is reached in the tens of thousands of packages before retention churn and the 2-char fan-out
  directories. Retention should therefore be capped by **file entries**, not only by generations and hours, with a
  build warning at 80% and a fail at the limit.

### 8. Serving publisher-authored blobs: Content-Type, sniffing, headers

- **Spec behaviour:** a supplied `text/plain`, `text/markdown` or `application/json` keeps its type; the HTML
  signature scan runs only for a missing/`unknown/unknown`/`application/unknown`/`*/*` type or when the server's
  type is trusted-unknown; the text-or-binary rules "never" yield a scriptable type. [spec]
  https://mimesniff.spec.whatwg.org/ (section 7, 7.1, 7.2)
- **GitLab Pages (source read, master branch, not measured on GitLab.com):** `serveFile` sets `Content-Type` from
  `detectContentType`: `mime.TypeByExtension(ext)`, then a custom map with only `.txt` -> `text/plain; charset=UTF-8`,
  then **sniffing the first 512 bytes with `http.DetectContentType`**. `X-Content-Type-Options: nosniff` is set only on
  the `_redirects` status page. [src] https://gitlab.com/gitlab-org/gitlab-pages/-/raw/master/internal/serving/disk/reader.go
  and https://gitlab.com/gitlab-org/gitlab-pages/-/raw/master/internal/serving/disk/helpers.go
  Go's HTML signatures are case-insensitive tag prefixes followed by space or `>` after leading whitespace, including
  `<!DOCTYPE HTML`, `<HTML`, `<HEAD`, `<SCRIPT`, `<!--` (and, per the Go source listing, `<H1`, `<DIV`, `<P`).
  [src, go1.21rc1 file] https://go.googlesource.com/go/+/refs/tags/go1.21rc1/src/net/http/sniff.go
  **Consequence:** an extensionless blob whose content starts `<p align="center">` is served `text/html; charset=utf-8`
  and runs script on the index origin. `.md` depends on the host's MIME table (Go's built-in table has no `.md`) and is
  **unverified**; `.txt` has an explicit mapping; `.json` is in Go's built-in table.
- **No custom headers:** "it is not possible to specify custom headers per project" and not on GitLab.com
  https://gitlab.com/gitlab-org/gitlab-pages/-/issues/50 [doc, undated]; the Pages docs document `_redirects`, not
  `_headers`. https://docs.gitlab.com/user/project/pages/introduction/ GitHub Pages documents no header mechanism and
  I could not verify its default headers (no live access). **Do not design around `nosniff`.**
- **GitHub Pages extensionless/unknown-extension behaviour: unverified.** Assumed `application/octet-stream` from
  general knowledge, which a browser downloads rather than renders.
- **Mitigations the ADR can mandate:**
  1. Every published blob path ends in an allow-listed extension from a fixed table (`.txt` for markdown text,
     `.json`, `.png`/`.webp`/`.jpg`/`.gif` for logos). Never `.html`, `.htm`, `.xhtml`, `.xml`, `.svg`, `.js`, `.mjs`,
     `.wasm`, `.pdf`; never extensionless. The extension comes from the table keyed by blob class, never from
     publisher input. (Today `LOGO_EXT = /^[A-Za-z0-9]{1,8}$/` in `src/enrich/index.ts` accepts `svg` and any
     alphanumeric extension; it guards path traversal, not type.)
  2. SVG logos: top-level navigation to an `image/svg+xml` file executes its script in the page's origin; `<img>`
     embedding does not. A public URL is navigable by anyone, so reject SVG for registry-sourced logos or rasterise.
     The marketplace logo path checks that the root element is `<svg` (`src/marketplace/read.ts`); I found no script
     sanitisation there.
  3. A **post-deploy header probe** in CI: `curl -sI` one sample per blob class from the live site and assert
     `content-type` is in the allow-list and is not `text/html` or `image/svg+xml`. Cheap, host-agnostic, catches a
     host change. [my inference]
  4. A custom domain for the index rather than a shared `<user>.github.io` origin, so script in a blob cannot reach
     other projects' storage. (General knowledge, not verified here.)
- JSON as a carrier is safe type-wise but costs escaping and a decode step; the ADR's own reason for raw blobs is
  removing base64 (33%), which `.txt` blobs also achieve.

### 9. Supply-chain angle: publisher-authored README/changelog content

- **Existing exposure, measured [probe]:** `@astrojs/markdown-remark` 7.3.1 (the copy under `grimoire/docs`; the
  indexer's own `node_modules` was not installed, its package.json requires astro ^6.0.1) rendered
  `<script>alert(1)</script>`, `<img src=x onerror=alert(2)>` and `[x](javascript:alert(3))` verbatim with default
  options. The indexer's `inlineConfig` (`src/renderer/index.ts`) sets only `markdown.shikiConfig`; no
  `rehype-sanitize`, no CSP found in the layouts. READMEs are rendered via `render(readmeEntry)` on
  `/p/<ns>/<name>/`. So publisher HTML already executes on the index origin today. Recommendation: sanitise at the
  Markdown stage (`rehype-sanitize` last in the chain, GitHub-like schema) and add a CSP `<meta>`; this is a
  pre-existing Block-tier issue independent of the layout ADR. (`rehype-sanitize` guidance:
  https://github.com/rehypejs/rehype-sanitize ; Astro states raw HTML in markdown works:
  https://docs.astro.build/en/guides/markdown-content/ , exact default not found in the docs, hence the probe.)
- **What raw blobs change:** (a) new directly navigable URLs with host-chosen types (finding 8); (b) **takedown
  latency**: a removed or malicious README stays reachable for 3 generations + 24 h and, via the CI cache, up to 7
  days. The ADR needs a purge path that overrides retention (delete from the retention ledger, publish a new
  generation, clear/bump the cache key); (c) base64-in-JSON was inert text to every reader; raw blobs are directly
  displayable by any future consumer.
- **Consumers:** terminal escape injection from remote metadata is a live CVE class: Composer CVE-2025-67746
  (published 2025-12-30) https://cvepremium.circl.lu/vuln/cve-2025-67746 ; GuardDog CVE-2026-44972, npm-check-updates
  CVE-2026-73035 (aggregator pages: https://vulert.com/vuln-db/guarddog-unsanitized-human-readable-scan-output-allows-terminal-escape-injection-from-malicious-pac-
  [3p]). Indirect prompt injection (external files fed to a model) is OWASP LLM01:2025; mitigations are segregating
  untrusted content and least privilege. [doc] https://genai.owasp.org/llmrisk/llm01-prompt-injection/
  The layout ADR keeps `c/b/` pipeline-private, so `grim` does not read blobs today: state the boundary as a rule
  ("blob text is untrusted display data: strip C0/ESC/OSC before a terminal, label as untrusted before an agent") so
  a later consumer inherits it.

## Sources

Primary or near-primary (all fetched this session unless marked):

- https://docs.github.com/en/actions/reference/workflows-and-actions/dependency-caching
- https://docs.github.com/en/actions/reference/security/secure-use
- https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax#cache-mode
- https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#schedule
- https://docs.github.com/en/pages/getting-started-with-github-pages/github-pages-limits
- https://github.blog/changelog/2025-11-20-github-actions-cache-size-can-now-exceed-10-gb-per-repository/
- https://github.blog/changelog/2026-06-26-read-only-actions-cache-for-untrusted-triggers/
- https://github.blog/changelog/2026-09-10-control-github-actions-cache-access-with-cache-mode
- https://github.com/actions/checkout/blob/main/CHANGELOG.md ; https://github.com/actions/checkout/blob/main/README.md
- https://github.com/actions/deploy-pages
- https://docs.gitlab.com/ci/caching/ ; https://docs.gitlab.com/ci/yaml/#cacheunprotect
- https://docs.gitlab.com/ci/pipelines/merge_request_pipelines/
- https://docs.gitlab.com/administration/instance_limits/ ; https://docs.gitlab.com/user/gitlab_com/
- https://docs.gitlab.com/user/project/pages/introduction/ ; https://gitlab.com/gitlab-org/gitlab-pages/-/issues/50
- https://gitlab.com/gitlab-org/gitlab-pages/-/raw/master/internal/serving/disk/reader.go and `.../helpers.go`
- https://theupdateframework.github.io/specification/latest/
- https://wiki.debian.org/DebianRepository/Format ; https://manpages.debian.org/trixie/apt/apt.conf.5.en.html
- https://conda.org/learn/ceps/cep-0016 ; https://doc.rust-lang.org/cargo/reference/registry-index.html
- https://github.com/opencontainers/image-spec/blob/main/descriptor.md
- https://mimesniff.spec.whatwg.org/ ; https://zlib.net/zlib_tech.html
- https://go.googlesource.com/go/+/refs/tags/go1.21rc1/src/net/http/sniff.go
- https://genai.owasp.org/llmrisk/llm01-prompt-injection/

Incident and research write-ups (secondary for facts, primary for the researcher's own claims):

- https://adnanthekhan.com/2024/05/06/the-monsters-in-your-build-cache-github-actions-cache-poisoning/ [>18mo]
- https://adnanthekhan.com/projects/cacheract [>18mo]
- https://adnanthekhan.com/posts/clinejection/
- https://safedep.io/tanstack-github-actions-cache-poisoning/ ; https://www.landh.tech/blog/20260511-tanstack-supply-chain-compromise/
- https://dev.to/jessehouwing/keep-the-creepy-crawlies-out-of-you-workflows-securing-github-actions-in-2026-2np0 [3p]
- https://gitlab.com/gitlab-org/gitlab/-/work_items/494478 [>18mo] ; https://gitlab.com/gitlab-org/gitlab/-/issues/547332
- https://gitlab.com/gitlab-org/gitlab/-/issues/506381 (snippet only) ; https://vulert.com/vuln-db/CVE-2022-1423 [>18mo, 3p]
- https://github.com/advisories/GHSA-76g3-38jv-wxh4 [>18mo] ; https://www.mend.io/vulnerability-database/CVE-2022-29173/ [>18mo, 3p]
- https://www.rapid7.com/db/modules/exploit/linux/http/ollama_rce_cve_2024_37032/ [>18mo, 3p]
- https://cvepremium.circl.lu/vuln/cve-2025-67746 ; https://bazel.build/remote/caching

Not fetched (snippet only or failed): Colin Watson's by-hash post, GitLab issue 506381, Cargo RFC status.

## Recommendation

Mandate, in this order (each maps to a finding above):

1. **Anchor.** All digests used to accept restored or fetched state come from the last published root closure over
   HTTPS; the cache is a byte source only; mismatch discards the cache copy. (1, 4)
2. **Write authority.** One trusted writer workflow per host; explicit `cache-mode: none|read` everywhere else;
   data-only, versioned, root-bound exact cache key; no shared key with any workflow that can run contributor
   influenced code. GitLab: protected/non-protected separation on, no `unprotect`, no cross-suffix fallback. (2, 3, 5)
3. **Bounded persistence.** Forced deep re-enrich after a maximum age (spread over buckets), an enricher/pipeline
   version in the input hash, and a `fresh` dispatch that ignores cache and published state; run the cold path in CI.
   (4, 5)
4. **Client verification.** Strict `sha256:` hex regex before any path/URL; stream-hash under a cap from the parent
   size; persist the new root only after the full closure verifies; compare generation before persisting. (4, 7)
5. **Generation.** Record always; soft ratchet per `(index, epoch)` on the client; `generation` derived from published
   state or a monotonic CI counter; no enforced `expires`. (6)
6. **Blob serving.** Allow-listed extension table, no extensionless or scriptable types, no SVG logos from registries,
   post-deploy header probe, and a purge path that overrides the 3-generation + 24 h retention. (8, 9)
7. **Host limits.** Retention capped by file entries with a build warning at 80%; count against 200,000 (GitLab.com)
   and 1 GB (both). (7)
8. **Out-of-axis, pre-existing, flag to owner:** sanitise README/changelog HTML in the renderer and add a CSP;
   verify the `validate` template against `actions/checkout` v7's fork-PR block and the reported 2026-11-02
   `pull_request_target` auto-disable; drop `cache: npm` from the `pull_request_target` template.

Confidence: high on GitHub/GitLab cache semantics and the 2026 platform changes (primary changelogs); medium on the
GitLab Pages sniffing consequence (source read on `master`, not measured on GitLab.com); low on GitHub Pages response
headers (not measured); the exact default-branch scope in the TanStack write-ups conflicts between sources.
