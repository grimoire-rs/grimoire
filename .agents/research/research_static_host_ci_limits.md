# Research: Hosting and CI limits that bound a statically hosted package index (GitHub Pages, GitLab Pages, hourly CI)

## Metadata

**Date:** 2026-10-20
**Axis:** operability (hosting and CI limits)
**Domain:** static hosting / CI
**Triggered by:** `/hex-architect` on `.agents/discussions/index-structure.md`
**Expires:** 2027-04-20
**Overlaps:** `research_static_site_search_at_scale.md` (Range, gzip, 1 GB, `max-age=600` measured 2026-10-20) and `research_catalog_sync_migrations.md` (Pages ETag/compression) cover the header behaviour. This file adds site/file-count budgets, CI limits and registry limits, and **corrects one claim** in both (see GitLab compression below).

Tags: [doc] vendor documentation, [src] host source code, [measured] header probe recorded in a sibling artifact, [3p] third party (weak), [derived] my arithmetic on stated assumptions. Items older than ~18 months are marked **OLD**.

**Limits of this run.** Bash network access was denied, so no header probe was run here. Every header claim is from docs, host source, or the sibling artifacts' earlier probes. No source gives deploy time versus file count for either host.

## Direct answer

- **GitHub Pages at 20,000 entries: does not fit as proposed; fits with changes.** The 1 GB site limit [doc] is enforced on an uncompressed tar [src]. The design's ~84 KB HTML page alone is ~1.7 GB raw at 20,000, and the whole site is ~1.9 GB with blobs kept off the host, ~3.7 GB with blobs published [derived]. The change that makes it fit: per-page HTML at or below ~35 KB raw, blobs cache-only, and retention of the large search file by generation count, not by a 24 h window.
- **GitLab Pages at 20,000: same 1 GB budget plus a second hard cap of 200,000 file entries, directories and symlinks included** [doc]. With blobs published the design is ~146k entries (73 % of the cap, no headroom past ~28k packages); with blobs cache-only ~66k.
- **100,000 ceiling: neither host.** Even with blobs off-host the tar is ~9.6 GB and GitLab needs ~318k entries [derived]. The frozen `all.json` alone (2 KB/entry) is ~200 MB, a fifth of the budget. That scale needs object storage plus a CDN, or no per-package HTML on the Pages site.
- **Biggest cross-cutting risk:** both hosts replace the whole site per deploy and re-stamp validators, so retention is a correctness need, and ETag revalidation of unchanged files does not work reliably across deploys.
- **CI limits are not the binding constraint.** GitHub: 6 h job, hourly cron allowed but only best-effort. GitLab Free: exactly 24 scheduled pipelines per day per schedule, i.e. hourly with zero spare.

## Trends

- GitHub raised the Actions cache ceiling beyond 10 GB on a pay-as-you-go basis (changelog 2025-11-20, Pro/Team/Enterprise only); the free default stays 10 GB and 7 days idle.
- The cache backend was rewritten and the legacy service sunset on 2025-02-01, so old `actions/cache` versions fail.
- GitHub is tightening unauthenticated access in general (changelog 2025-05-08, no numbers, does not name ghcr.io or github.io).
- GitLab has an open proposal to cut the Pages file cap per tier for new projects (Free 30,000, Premium 100,000, Ultimate 200,000). Status unverified.
- Pages deploy reliability has had incidents (Jan 2026 threads: `deployment_queued` then 10 min timeout despite low usage).

## Key findings

### GitHub Pages [doc unless tagged]

| Item | Finding | Source |
|---|---|---|
| Site size | Published site <= 1 GB; source repo 1 GB recommended | [limits](https://docs.github.com/en/pages/getting-started-with-github-pages/github-pages-limits) |
| Bandwidth | Soft 100 GB/month; over-quota sites "may not be served" | same |
| Builds | Soft 10/hour, **not applied** to a custom Actions workflow | same |
| Deploy timeout | 10 min; `deploy-pages` `timeout` default 600000 ms, `error_count` 10 | same; [deploy-pages](https://github.com/actions/deploy-pages) |
| File count | **No limit documented** anywhere I found | absence in the above |
| Rate limit | Throttled requests get 429 with an HTML body; no number | limits page |
| Artifact | `upload-pages-artifact` builds an **uncompressed** `tar -cvf` (no `-z`), then `upload-artifact`; README: tar "under 10GB (we recommend under 1 GB)", supported maximum 1 GB, larger "often" fails on the 10 min timeout; no symlinks or hard links; dotfiles excluded unless `include-hidden-files`; `retention-days` default 1 | [action.yml](https://raw.githubusercontent.com/actions/upload-pages-artifact/main/action.yml) [src], [README](https://github.com/actions/upload-pages-artifact) |
| Tar overhead | 512 B header per file and per directory plus padding to 512 B: 34 MB at 42k files / 23k dirs, 75 MB at 122k files [derived] | tar format |
| Cache-Control | `max-age=600` on responses probed (HTML and octet-stream); GitHub does not document it | [measured] `research_static_site_search_at_scale.md` s.147; [3p 2026-08](https://dev.to/jangwook_kim_e31e7291ad98/your-last-deploy-reset-every-cache-validator-on-the-site-4dd3). One 3p tool page claims 1 year for "versioned assets"; unverified and contradicted by the probes |
| ETag | Undocumented. One 3p probe decodes it as mtime plus size, with the Last-Modified shared by all files of a deploy. So a redeploy of identical bytes likely changes every validator **[3p, inferred, not observed on Pages]** | same dev.to post |
| Compression | gzip served (HTML, and `application/octet-stream` observed); no brotli source found | [measured] sibling artifact; [3p](https://habr.com/en/articles/841754) |
| Range | 206 supported, but with `Accept-Encoding: gzip` the offsets index the **gzip stream** | [measured] sibling artifact |
| Many small files | No source relates deploy time to file count. Staff said (2022, **OLD**, from a search snippet) that ~10 GB takes more than the 10 min timeout | [#35197](https://github.com/orgs/community/discussions/35197) |
| Reliability | Jan 2026: deploys stuck in `deployment_queued` then timed out; duplicate-artifact failure on re-run | [#184211](https://github.com/orgs/community/discussions/184211), [deploy-pages#338](https://github.com/actions/deploy-pages/issues/338) |

### GitHub Actions [doc]

- **Job limit** 6 h on GitHub-hosted runners; workflow run 35 days; re-runs 50 per run. [limits](https://docs.github.com/en/actions/reference/limits)
- **Cache** 10 GB per repo by default, now raisable (changelog above) for Pro/Team/Enterprise at pay-as-you-go; entries idle 7 days are evicted; at the cap, least-recently-accessed first; 200 uploads and 1,500 downloads per minute per repo; keys up to 512 chars. No per-entry size cap is documented. [dependency caching](https://docs.github.com/en/actions/reference/workflows-and-actions/dependency-caching), [changelog 2025-11-20](https://github.blog/changelog/2025-11-20-github-actions-cache-size-can-now-exceed-10-gb-per-repository/)
- **Keys are write-once:** a matching key creates no new cache; restore uses exact key, then `restore-keys` prefix (most recent match); default-branch caches are readable from other branches. [actions/cache](https://github.com/actions/cache)
- **Scheduled workflows:** shortest interval 5 min; "can be delayed during periods of high loads"; "High load times include the start of every hour"; "some queued jobs may be dropped"; run only on the default branch's latest commit; **disabled after 60 days without repository activity in a public repo**; optional IANA `timezone`. [events](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#schedule)
- **Observed delay:** 15+ min near the hour in 3p posts; an 8-14 h outlier with one dropped run (Jul 2026), no staff reply. [3p/community #201738](https://github.com/orgs/community/discussions/201738)
- **Cost:** standard-runner usage is free for public repos; the billing page states storage allowances only for private repos. [billing](https://docs.github.com/en/billing/concepts/product-billing/github-actions)

### GitLab Pages and CI [doc unless tagged]

| Item | Finding | Source |
|---|---|---|
| Site size | GitLab.com 1 GB; self-managed default 100 (MB), settable | [gitlab_com](https://docs.gitlab.com/user/gitlab_com/), [admin/pages](https://docs.gitlab.com/administration/pages/) |
| What counts | Uncompressed: a deploy failed with "artifacts for pages are too large: 1459447313" on small compressed artifacts. **OLD (2018-2021 reports), not re-verified** | [#2823](https://gitlab.com/gitlab-org/gitlab/-/issues/2823) |
| File entries | **200,000 per site, counting directories and symlinks**; default on GitLab.com and self-managed (`pages_file_entries`, admin only) | [instance limits](https://docs.gitlab.com/administration/instance_limits/) |
| Proposal | Tiered file cap Free 30,000 / Premium 100,000 / Ultimate 200,000, new projects only; shipped status unknown | [#500273](https://gitlab.com/gitlab-org/gitlab/-/issues/500273) |
| Artifacts | GitLab.com max 1 GB compressed; default expiry 30 days; the Pages size is bounded by the artifact max | [gitlab_com](https://docs.gitlab.com/user/gitlab_com/) |
| Deploy | Full replace per deployment; `pages.expire_in`; up to 1,000 parallel (`path_prefix`) deployments per namespace; deploy needs a non-empty root `index.html` | [Pages](https://docs.gitlab.com/user/project/pages/), [introduction](https://docs.gitlab.com/user/project/pages/introduction/) |
| Storage | One zip per deployment; a path lookup reads the zip index, so directory fan-out has no lookup cost [src, inference]; every directory costs one entry against the 200,000 | [admin/pages](https://docs.gitlab.com/administration/pages/) |
| Cache-Control | `max-age=600` plus `Expires` (+10 min), only when access control is off | [reader.go](https://gitlab.com/gitlab-org/gitlab-pages/-/raw/master/internal/serving/disk/reader.go) [src] |
| ETag | `sha[-encoding]` where `sha` is a **per-deployment** `Source.SHA256`, not per-file content; Last-Modified from the zip entry mtime. So all validators change on any new deployment (what the sha hashes is not stated: inferred to be the archive) | reader.go, [serving.go](https://gitlab.com/gitlab-org/gitlab-pages/-/raw/master/internal/vfs/serving/serving.go), [lookup_path.go](https://gitlab.com/gitlab-org/gitlab-pages/-/raw/master/internal/source/gitlab/api/lookup_path.go) [src] |
| Compression | Server never compresses; serves a `.br` (preferred) or `.gz` sibling when the client accepts it. **No extension allowlist in the server** [src helpers.go]; the `htm,html,xml,txt,text,js,css,svg` list is only the docs' sample CI job. So `.json.gz` siblings work, which corrects the "JSON not eligible" reading in two sibling artifacts. Unconfirmed: the `Content-Type` sent for a `.json.gz` sibling. Each sibling is one more entry | [helpers.go](https://gitlab.com/gitlab-org/gitlab-pages/-/raw/master/internal/serving/disk/helpers.go), [docs](https://docs.gitlab.com/user/project/pages/introduction/) |
| Range | Ignored for compressed and zip-VFS paths | helpers.go, serving.go; [measured] sibling artifact |
| Pages request limits | 1,000 requests / 50 s per IP; 5,000 / 10 s per Pages domain | [rate limits](https://docs.gitlab.com/user/gitlab_com/rate_limits/) |
| Job timeout | Instance default 60 min; per project 10 min to 1 month. GitLab.com hosted-runner cap not found | [CI limits](https://docs.gitlab.com/administration/cicd/limits/) |
| Schedules | GitLab.com cron `*/5 * * * *`; **24 pipelines/day per schedule on Free, 288 on paid**; 10 schedules (Free) / 50 (paid) per project; self-managed worker default `3-59/10`; docs advise staggering | [gitlab_com](https://docs.gitlab.com/user/gitlab_com/), [schedules](https://docs.gitlab.com/ci/pipelines/schedules/) |
| Cache | Max 4 caches per job; GitLab.com uses a distributed S3 cache; **size and retention not documented** | [caching](https://docs.gitlab.com/ci/caching/) |

### Registries probed once per package per hour [doc unless tagged]

| Registry | Quota | Does a manifest HEAD count? |
|---|---|---|
| Docker Hub | Unauthenticated 100 pulls / 6 h per IPv4 or IPv6 /64; Personal 200; paid unlimited; 429 when over | **No:** "Using GET ... counts towards the limit. Using HEAD won't"; version checks do not count. [pulls](https://docs.docker.com/docker-hub/usage/pulls/). A separate, undocumented abuse throttle (~2,000-3,000 req/min) is **OLD** [3p Dec 2024](https://www.augmentedmind.de/2024/12/15/docker-hub-rate-limit-head-request/) |
| GHCR | **No pull, HEAD or manifest limit documented.** Only layer (10 GB) and upload-time (10 min) limits | Undocumented. 429s reported under parallel pushes, no staff reply (**OLD**, Aug 2024) [#134682](https://github.com/orgs/community/discussions/134682); [container registry docs](https://docs.github.com/en/packages/working-with-a-github-packages-registry/working-with-the-container-registry) |
| GitLab registry | No pull quota documented; 15 min JWT; an IP is banned 15 min after 300 failed auth requests per minute (Git and registry combined); successful auth resets it; `gitlab-ci-token` exempt | Undocumented; a HEAD needs a JWT, so reuse one token per repository scope. [rate limits](https://docs.gitlab.com/user/gitlab_com/rate_limits/), [gitlab_com](https://docs.gitlab.com/user/gitlab_com/) |

Load shape: 20,000 HEADs/hour is ~5.6 per second sustained; at pool 8 and an assumed 300 ms each, ~12 min [derived].

### Budget arithmetic for the proposed design [derived]

Assumptions: HTML 84 KB (from the brief); record 2 KB; search 1 KB/entry; `all.json` 2 KB/entry (a 19-entry real sample is 1.2 KB); blobs 84 KB/package unshared (upper bound); 0.15 namespace directories per package; 3 retained generations with every shard rewritten (upper bound); hourly rewrite of the search file.

| Scenario | Files | Dirs | GitLab entries | Tar size |
|---|---|---|---|---|
| 20,000, blobs published, 3 gens | 122k | 24k | 146k (73 % of cap) | 3.7 GB |
| 20,000, blobs cache-only, 3 gens | 42k | 23k | 66k | 1.9 GB |
| 20,000, blobs cache-only, search file kept 24 h | 42k | 23k | 66k | 2.35 GB (+0.43 GB from 21 extra 20 MB copies) |
| 100,000, blobs cache-only, 3 gens | 202k | 0.115M | 318k | 9.6 GB |
| 100,000, blobs published | 602k | 116k | 718k | 18.4 GB |

- **Entries per package:** ~7.2 with blobs published (cap reached near 28k packages), ~3.2 cache-only (near 63k).
- **Per-page HTML budget at 1 GB:** non-HTML content at 20,000 is ~196 MB (`all.json` 40 MB, 3 search copies 60 MB, records ~50 MB, shards ~18 MB, headers and directories ~46 MB), leaving **~39 KB per page**. At 100,000 it is **~0.3 KB**, i.e. none.
- **Compression** (measured here on 12 real packages' enrich files): readme 2.1x, changelog 2.75x, ~2.5x overall; a 19-entry `all.json` 4.2x. Small samples. A page is ~34 KB on the wire, so 100 GB/month is ~3M page views.
- **Search file:** ~20 MB raw, ~4 MB gzip (the discussion's estimate scaled to 20,000), so the 100 GB soft limit is ~25,000 cold loads/month. `all.json`: ~5-10 MB gzip; if validators reset each deploy (above), an always-on hourly poller costs ~3.6-7 GB/month, so ~14-28 such clients exhaust the soft limit.
- **Cache:** blobs are ~1.7 GB raw (~0.7 GB compressed) at 20,000. A new key per hourly run is ~17 GB/day of uploads against a 10 GB cap (thrash). Key on the blob-set digest and save only on change.
- **Cold build** (cache lost): 20,000 packages x ~5 registry requests at 300 ms, pool 8, is ~1 h [derived, assumed latency]. That is inside GitHub's 6 h and GitLab's 60 min default, but nothing else in the same job budget.

## Sources

Docs: [GitHub Pages limits](https://docs.github.com/en/pages/getting-started-with-github-pages/github-pages-limits); [upload-pages-artifact README](https://github.com/actions/upload-pages-artifact) and [action.yml](https://raw.githubusercontent.com/actions/upload-pages-artifact/main/action.yml); [deploy-pages](https://github.com/actions/deploy-pages); [Actions limits](https://docs.github.com/en/actions/reference/limits); [dependency caching](https://docs.github.com/en/actions/reference/workflows-and-actions/dependency-caching); [cache size changelog](https://github.blog/changelog/2025-11-20-github-actions-cache-size-can-now-exceed-10-gb-per-repository/); [actions/cache](https://github.com/actions/cache); [schedule event](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#schedule); [Actions billing](https://docs.github.com/en/billing/concepts/product-billing/github-actions); [unauthenticated limits changelog 2025-05-08](https://github.blog/changelog/2025-05-08-updated-rate-limits-for-unauthenticated-requests/); [GitLab.com settings](https://docs.gitlab.com/user/gitlab_com/); [GitLab.com rate limits](https://docs.gitlab.com/user/gitlab_com/rate_limits/); [instance limits](https://docs.gitlab.com/administration/instance_limits/); [CI/CD limits](https://docs.gitlab.com/administration/cicd/limits/); [Pages admin](https://docs.gitlab.com/administration/pages/); [Pages introduction](https://docs.gitlab.com/user/project/pages/introduction/); [CI caching](https://docs.gitlab.com/ci/caching/); [pipeline schedules](https://docs.gitlab.com/ci/pipelines/schedules/); [Docker Hub pulls](https://docs.docker.com/docker-hub/usage/pulls/); [GHCR docs](https://docs.github.com/en/packages/working-with-a-github-packages-registry/working-with-the-container-registry).
Source: gitlab-pages [reader.go](https://gitlab.com/gitlab-org/gitlab-pages/-/raw/master/internal/serving/disk/reader.go), [helpers.go](https://gitlab.com/gitlab-org/gitlab-pages/-/raw/master/internal/serving/disk/helpers.go), [serving.go](https://gitlab.com/gitlab-org/gitlab-pages/-/raw/master/internal/vfs/serving/serving.go), [lookup_path.go](https://gitlab.com/gitlab-org/gitlab-pages/-/raw/master/internal/source/gitlab/api/lookup_path.go).
Weak: [dev.to ETag/deploy post](https://dev.to/jangwook_kim_e31e7291ad98/your-last-deploy-reset-every-cache-validator-on-the-site-4dd3); [GitLab #2823](https://gitlab.com/gitlab-org/gitlab/-/issues/2823) (**OLD**); [GitLab #500273](https://gitlab.com/gitlab-org/gitlab/-/issues/500273); community discussions [#35197](https://github.com/orgs/community/discussions/35197) (**OLD**, snippet only), [#184211](https://github.com/orgs/community/discussions/184211), [#201738](https://github.com/orgs/community/discussions/201738), [#134682](https://github.com/orgs/community/discussions/134682) (**OLD**); [deploy-pages#338](https://github.com/actions/deploy-pages/issues/338).
Local measurements: gzip ratios from the checked-out sample index data and a Python size model, both run in this session.

## Recommendation

1. **Size the HTML page to the host, not the profile.** Cap the embedded README + changelog so each page is <= ~35 KB raw at 20,000 (truncate the changelog and link out). Both hosts' 1 GB is a hard wall at the tar level, and the 84 KB profile is ~1.7 GB by itself.
2. **Blobs cache-only, not published.** Published blobs push the site to ~3.7 GB and to 73 % of GitLab's entry cap. Treat a cold cache as a normal event: the build must finish a full re-enrich inside the job budget (about an hour at the assumed latency), and on GitLab.com the cache has no documented retention.
3. **Retention by generation count for big files, time window only for small ones.** Keep the 24 h rule for shards, records and plane indexes (~100 KB per release) but keep only the last 3 generations of the search file; the 24 h rule on it costs +0.43 GB.
4. **Treat 100,000 as a host change, not a tuning problem.** State in the ADR that Pages is the 20,000 design point's host; the ceiling needs object storage plus a CDN. Keep paths relative so that move stays a config change.
5. **Do not rely on ETag across deploys on either host.** Clients should cache content-addressed paths by digest and never revalidate them (browser `cache: 'force-cache'`); only `c/root.json` is revalidated. A deploy that changes nothing (unchanged tree hash) should be skipped.
6. **Schedule at an off-hour minute** (for example `17 * * * *`), make the run idempotent, add a `workflow_dispatch` path, keep a real commit cadence (public-repo schedules are disabled after 60 days of no activity), and retry a failed deploy once (Jan 2026 incident class). On GitLab Free, hourly is exactly the 24/day cap.
7. **Registry probing is fine at this load** on Docker Hub (HEAD uncounted), GHCR (no documented limit) and GitLab (reuse one JWT per repository). Keep the pool at 8, honour `Retry-After`, and log 429s per host.
8. **One spike settles four unknowns.** On a throwaway repo per host, deploy a synthetic 20,000-package tree (about 2 GB tar, about 66k entries). Record: is a >1 GB tar accepted and how long does upload plus deploy take; does ETag survive a redeploy of identical bytes; `Content-Type` and `Content-Encoding` for `.json` and `.json.gz`; and `max-age`.
