# Research: Catalog sync layouts and dual-serving migrations

## Metadata

**Date:** 2026-10-17
**Domain:** packaging
**Triggered by:** discussion `.agents/discussions/index-scale-rollout.md`
**Expires:** 2027-04-17

## Scope

How comparable products structure and sync large catalogs, how new clients discover a new layout, how long both layouts were served, and what was measured. Question framing: static, Git/Pages-hosted catalog indexes, a 20,000-entry design point, many clients polling hourly, flat JSON feed must keep working for old clients. Evidence for and against only; no recommendation.

Source quality tags: **[doc]** vendor or spec documentation, **[src]** vendor source code, **[blog]** vendor announcement, **[3p]** third-party or single-author observation (weak).

## Research: catalog-sync-migrations

### A. Agent-config and extension catalogs

**Claude Code plugin marketplaces** [doc]
- Catalog = one `marketplace.json` (`name`, `owner`, `plugins[]`); no documented cap on entry count or file size for git-hosted marketplaces. Each plugin entry validated independently, so one invalid entry does not fail the file. https://code.claude.com/docs/en/plugins/marketplace-reference
- Forward compatibility: "Claude Code ignores an unknown top-level key or plugin-entry key rather than rejecting it" (same page). Additive evolution without a version gate.
- Hosting modes: git repo (GitHub/GitLab/other), a bare `marketplace.json` URL, or a shared directory. URL mode downloads only that file: limit **5 MiB**, **10 s** server response, redirect restrictions; relative-path plugin sources do not work in URL mode. https://code.claude.com/docs/en/plugins/host-marketplace
- Git mode: whole-repo clone; timeout default **120 s** (`CLAUDE_CODE_PLUGIN_GIT_TIMEOUT_MS` raises it), and the troubleshooting page says the re-clone used for updates "can time out on large repositories". Background auto-update checks for new commits, and on new commits (or on auth/reach failure) "clones the marketplace again and replaces the existing checkout". Marketplace source supports `sparsePaths` (sparse checkout). https://code.claude.com/docs/en/plugins/host-marketplace , https://code.claude.com/docs/en/plugins/troubleshooting
- Update cadence: auto-update off by default for third-party marketplaces; when on, runs after the first message of a session plus a random delay of up to 10 minutes. Install of `name@marketplace` refreshes first unless refreshed within the last 30 s. https://code.claude.com/docs/en/plugins/loading
- Per-plugin versioning: a plugin updates only when its computed version changes (manifest `version`, else entry `version`, else commit SHA). Catalog-level "what changed" is not exposed; there is no delta or ETag mechanism documented.
- Renames/removals: append-only `renames` map and `forceRemoveDeletedPlugins` handle catalog-level compatibility for existing installs. https://code.claude.com/docs/en/plugins/host-marketplace
- Org management: managed settings `extraKnownMarketplaces`, `enabledPlugins`, `strictKnownMarketplaces` (allowlist by repo/url/hostPattern), `blockedMarketplaces`, `autoUpdate` per marketplace; seed directory (`CLAUDE_CODE_PLUGIN_SEED_DIR`) for containers/CI with no clone at runtime; "entries carry no audience field, and every user who adds the marketplace sees the whole catalog. Host separate marketplaces for separate audiences." https://code.claude.com/docs/en/plugins/org
- Release channels = two marketplaces with different `name`; "Claude Code has no release-channel concept".

**GitHub Copilot (CLI / VS Code plugins)** [doc]
- Same shape: `marketplace.json` looked up in order `marketplace.json`, `.plugin/`, `.github/plugin/`, `.claude-plugin/`; the Claude path is read as a compatibility location. Name limits (64/64/1024 chars) documented; no plugin-count or total-size limit stated. Fetch mechanism (git clone or not) and refresh interval are not documented; manual `copilot plugin marketplace update`, optional `autoUpdate: true`, cache under `~/.cache/copilot/marketplaces/`. https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-plugin-reference
- Enterprise-managed marketplaces defined in `.github-private/.github/copilot/settings.json` and pulled automatically by VS Code and CLI for licensed users; caching/refresh not described. https://github.blog/changelog/2026-06-05-enterprise-managed-plugins-in-vs-code-in-public-preview/

**Official MCP Registry** [doc]
- Not designed for direct host consumption: "intended to be consumed primarily by downstream aggregators"; aggregators "scrape data on a regular but infrequent basis (e.g., once per hour), and persist the data in their own data store"; "does not provide uptime or data durability guarantees". https://modelcontextprotocol.io/registry/about , https://modelcontextprotocol.io/registry/registry-aggregators
- API: `GET /v0.1/servers` with cursor pagination (`limit`, opaque `nextCursor` in `metadata`) and `updated_since` (RFC 3339) for deltas; per-server and per-version endpoints; `status` field (`deprecated`/`deleted`) is the only mutable field, so aggregators track status for removals. With `updated_since`, `include_deleted` defaults to true (OpenAPI doc). https://modelcontextprotocol.io/registry/registry-aggregators , https://raw.githubusercontent.com/modelcontextprotocol/registry/main/docs/reference/api/official-registry-api.md
- Subregistry = an aggregator that also implements the registry OpenAPI spec; may add `_meta` fields under a reverse-DNS key. Subregistries "may optionally require authentication". No documented ETag/Cache-Control guidance; no documented max `limit`.
- Layout migration evidence: path-versioned API (`/v0` to `/v0.1`); a README note dated 2025-10-24 declares an "API freeze (v0.1)" with no breaking changes for a month or more while v0 development continues [3p mirror of README; official dated note not fetched directly]. Registry stays in "preview: breaking changes or data resets may occur". No documented dual-serving window or sunset date for the earlier path.
- Self-hosting: "the official MCP Registry codebase is not designed for self-hosting".

**VS Code Marketplace / Open VSX** [src, doc]
- VS Code does not sync a catalog: the client POSTs queries to the gallery (`extensionquery`) and pages results; defaults seen in client source: query `pageSize` default 50 (state default 10); update check sends all installed extensions in one query (`withPage(1, extensionInfos.length)`), no chunking. Request header `Accept-Encoding: gzip`; request timeout default 60 s; no ETag/If-None-Match/Cache-Control logic found; any 4xx from the query endpoint returns an empty result (no 429-specific handling). https://raw.githubusercontent.com/microsoft/vscode/main/src/vs/platform/extensionManagement/common/extensionGalleryService.ts
- The Microsoft gallery API is undocumented publicly; community payload examples show `pageNumber`, `pageSize`, `flags` bitmask, per-version `lastUpdated` [3p]. I found no source confirming a max page size or rate limit.
- Open VSX: public registry API documented only through a generated Swagger UI (not retrievable as text); wiki says user/admin API parts are "internal and only to be used by the web UI". https://github.com/eclipse/openvsx/wiki/Registry-API

**JetBrains plugin repository** [doc]
- Custom repository = one or more static `updatePlugins.xml` files (any filename) listing every hosted plugin's latest version; HTTPS required; "more than one updatePlugins.xml file may be required" when IDE versions differ ("Multiple plugins with the same `id` but different `idea-version` attributes must be split into separate updatePlugins-*.xml files", each its own URL). IDE build is passed as `build` parameter, usable for server-side filtering. Update check default every 24 h [3p community answer]. Doc says nothing on caching or size. https://plugins.jetbrains.com/docs/intellij/custom-plugin-repository.html
- Pattern relevant to sharding by consumer capability: per-IDE-version feed files.

### B. Package-ecosystem migrations

**Cargo: git index to sparse index** [doc, blog]
- Discovery: by URL scheme, not probing. Sparse registries use the `sparse+` prefix (`sparse+https://index.crates.io/`); git has no prefix. Root `config.json` holds `dl`, optional `api`, `auth-required`; Cargo fetches `config.json` first, and on HTTP 401 re-requests with the token. https://doc.rust-lang.org/cargo/reference/registry-index.html
- Per-crate files over HTTP at `/{2}/{2}/{name}` style paths; cached with `ETag` or `Last-Modified`; sends `If-None-Match`/`If-Modified-Since`; ETag preferred when both present; 304 saves bandwidth. Docs recommend CDN cache invalidation on update, otherwise "users may not be able to access new crates until the cache is cleared". Stale-miss handling: cargo may retry with a cache-buster query parameter (RFC). https://rust-lang.github.io/rfcs/2789-sparse-index.html
- Compression: RFC says servers can serve pre-compressed files; largest file "compresses from 1MiB to 26KiB with Brotli".
- Sizes (RFC): full index 176 MiB uncompressed tarball, 16 MiB gzip -1; git clone reported 215 MiB; per-file worst case about 39 MiB compressed. HTTP/2 about twice as fast as HTTP/1.1 for the many small requests.
- Dual serving: git index is the source of truth and "Cargo will continue to support git indexes indefinitely"; `index.crates.io` serves the same data over a different transport. https://blog.rust-lang.org/inside-rust/2023/01/30/cargo-sparse-protocol/ ; spec warns "it's not recommended to offer a registry with both protocols" for third-party registries because the URL is in the lockfile, except crates.io where "Cargo internally substitutes the equivalent git URL". Switching protocols changes the cache path and re-downloads crates (1.70 notes).
- Timeline: call for testing nightly 2022-06-21; stabilized in 1.68 (2023-03-09, opt-in for crates.io); default flipped in PR [rust-lang/cargo#11791](https://github.com/rust-lang/cargo/pull/11791) merged 2023-03-05, shipped 1.70 (2023-06-01), after about 6 weeks nightly + 6 weeks beta. Opt-out `registries.crates-io.protocol = "git"`. About 11 months from first nightly to default; git index still served.
- Outcomes: official posts give no figures ("significant performance improvement"). Measured by users: `cargo fetch --locked` 75 s to about 10 s at 100 Mbit/s (single downstream commit, [rust-lang/cargo#11791](https://github.com/rust-lang/cargo/pull/11791)); CI savings 40-80 s per run; a Bazel integration resolve 2-4 min to under 10 s; warm-cache local `cargo update` 0.3 s (git) vs 1.0 s (sparse) in the first test (sparse was slower when nothing needed fetching). https://internals.rust-lang.org/t/call-for-testing-cargo-sparse-registry/16862 , https://internals.rust-lang.org/t/cargo-sparse-protocol-feedback-thread/18234
- Failure reports: CDN max-age 10 min delayed new crates, cut to 1 min with invalidation on publish planned; publish timeouts (60 s wait, serial index updates), a 429 when publishing about 8 crates back to back; third-party tools (cargo-deny, cargo upgrade) still on git index; no proxy/firewall reports despite asking. Same threads.

**Homebrew: git taps to JSON API** [blog, src]
- Opt-in `HOMEBREW_INSTALL_FROM_API` in 3.3.0 (2021-10-25); default in 4.0.0 (2023-02-16) with opt-out `HOMEBREW_NO_INSTALL_FROM_API=1`; auto-update interval "every 24 hours rather than every 5 minutes". Posts give no measured speedups. https://brew.sh/2023/02/16/homebrew-4.0.0/ , https://brew.sh/blog/
- 4.1.0 (2023-07-20): signed endpoint with client signature verification, cached-bottle fallback on failed freshness check, auto-update from API only for commands that auto-updated from git before. https://brew.sh/2023/07/20/homebrew-4.1.0/
- Source: `.jws.json` payloads verified (PS512 RSA-PSS, key `homebrew-1`); failure deletes the file and aborts with "Potential MITM attempt detected". Freshness: API TTL default **450 s** (`HOMEBREW_API_AUTO_UPDATE_SECS`), 7-day stale default when auto-update off, unsigned per-resource responses revalidated hourly. Conditional fetch: curl `--etag-save`/`--etag-compare` (ETag stored with a companion `.url` file), fallback to `--time-cond` on older curl; retry unconditionally on curl exit 56. On download failure with default domain: use cached file ("update failed, falling back to cached version"); with a custom `HOMEBREW_API_DOMAIN` mirror: retry once against the default domain. https://raw.githubusercontent.com/Homebrew/brew/HEAD/Library/Homebrew/api.rb , https://raw.githubusercontent.com/Homebrew/brew/HEAD/Library/Homebrew/env_config.rb
- 6.0.0 (2026-06-11): default "internal" JSON API that "combines all Homebrew's metadata into a single download"; was opt-in via `HOMEBREW_USE_INTERNAL_API` since 5.0.0. No size or speed figures in the post. https://brew.sh/2026/06/11/homebrew-6.0.0/
- Pattern: three successive layouts (git, per-item JSON, single combined file), each introduced opt-in for one or more major versions, with env-var escape hatches.

**PyPI Simple API (PEP 503 HTML to PEP 691 JSON)** [spec]
- Discovery by content negotiation on the same URL: `Accept: application/vnd.pypi.simple.vN+json`; `text/html` aliased to `application/vnd.pypi.simple.v1+html`; PEP says "MUST not break existing clients that are accessing the API as a strictly PEP 503 API" and suggests clients weight `text/html` at `q=0.01`. Separate-URL JSON was rejected because it complicates the root URL and discovery. https://peps.python.org/pep-0691/
- The PEP itself notes static hosts differ: "S3 allows custom types but no negotiation, and GitHub Pages doesn't allow custom types"; offers separate endpoints per format (`/simple/v1+html/`) as the escape hatch.
- Version evolution after the format switch is additive: 1.0 initial; 1.1 `versions`, `size`, `upload-time` (PEP 700); 1.2 tracks (PEP 708); 1.3 provenance (PEP 740); 1.4 project status (PEP 792). Rules: clients "MUST ignore keys that they don't understand"; higher minor version = warn; higher major = fail. https://packaging.python.org/en/latest/specifications/simple-repository-api/
- PEP has no ETag/caching guidance and no measured outcomes; HTML is still served (PyPI "no plans to drop HTML at the time of writing").

**npm: abbreviated metadata and replication feed** [doc, blog]
- Abbreviated packument: `Accept: application/vnd.npm.install-v1+json` returns subset (name, modified, dist-tags, versions); no Accept header returns the full document; some full documents "over 10MB uncompressed". https://github.com/npm/registry/blob/main/docs/responses/package-metadata.md
- Replication feed cut-over (CouchDB `_changes` to new service): new endpoints live **2025-03-18** alongside old ones, opt-in header `npm-replication-opt-in: true`, brownouts from 2025-04-02, old endpoints "fully deprecated" **2025-05-29** (about 10 weeks parallel), after which "all requests ... will automatically be served by the new APIs, regardless of header usage". https://github.blog/changelog/2025-02-27-changes-and-deprecation-notice-for-npm-replication-apis/ , https://github.blog/changelog/2025-04-22-reminder-for-changes-to-npm-replication-feeds-apis/
- Community-reported (thread, not vendor-measured): only `_changes`, `_all_docs`, root remain; streaming modes removed, clients paginate by `since`; `limit` default 1000, max 10000; sequence numbers jumped from about 20M to 40M to 60M; 503s on both old and new during rollout; a weekend pause of the opt-in API; about 27k packages 404 despite valid feed entries; requests without the header got 400 during the window. https://github.com/orgs/community/discussions/152515

**Helm: index.yaml to OCI** [doc, 3p]
- OCI support GA (no longer experimental) in Helm 3.8.0; Helm docs recommend OCI registries but index.yaml repos remain supported (no removal stated). The registry doc has "not yet been updated for Helm 4". https://helm.sh/docs/topics/registries/
- Scaling symptoms of a monolithic index [3p, issue trackers]: Bitnami index about 13.4 MB driving about 2 GB peak in Flux source-controller until caching (about 200 MB after) [fluxcd/source-controller#303]; Helm 3.17.3 added a decompressed size cap (5 MB error seen with the Bitnami index) [fluxcd/helm-controller#1264]; ChartMuseum index "tens of megabytes" failing `helm repo update` until timeouts/gzip at the proxy; Nexus rejects large YAML by default (`nexus.helm.yaml.max.bytes`); GitLab's Helm index endpoint silently returned only the 1,000 most recent charts [gitlab-org/gitlab#579785]. Aggregated via search results, not individually fetched; treat as leads.
- Mitigation by migration: Flux says OCI repositories remove the index download entirely.

**Go module proxy and index** [doc]
- Layered, immutable-by-design layout: `$base/$module/@v/list|.info|.mod|.zip|@latest`; `.mod`/`.zip` "must always serve the same content"; `list` and `.info` may change so clients refetch. https://go.dev/ref/mod
- Discovery/fallback is client config, not probing: `GOPROXY=a,b` falls through only on 404/410; `a|b` falls through on any error. Proxy list is the dual-serving mechanism.
- Change feed: `index.golang.org/index?since=<RFC3339>&limit=<n>` newline-delimited JSON, chronological, limit default and max 2000, `include=all` for versions ever served. Bulk consumers directed to a cached-only mode of the proxy. https://index.golang.org/
- Checksum DB uses tile endpoints `tile/$H/$L/$K[.p/$W]` with partial-tile fallback to full tile (append-only log layout, cache-friendly). No cache-lifetime or measured figures found in the reference.

**TUF** [spec]
- Roles: timestamp (small, frequently re-signed, hash of snapshot), snapshot (versions of all targets metadata), targets (and delegations). Consistent snapshots: versioned metadata filenames (`42.targets.json`) and hash-prefixed target filenames, so readers of an older snapshot are not disrupted; `timestamp.json` is the only unprefixed file. https://theupdateframework.github.io/specification/latest/
- Client update order: root, timestamp, snapshot, targets, with application-defined byte limits per metadata type. Delegations scale via succinct hashed bins (SHA-256 prefix to bin). Search limit on delegation depth.
- Relevance is structural (small pointer file that changes often + immutable content-addressed bodies); the spec gives no migration or measured data.

**Debian/APT** [doc]
- `Acquire-By-Hash: yes` in the signed Release file advertises per-hash index paths (`by-hash/<HASHTYPE>/<hash>`); canonical name stays and links to latest; "current version must and two or more previous versions of a file should be available"; client may fall back to canonical location if by-hash fails. Introduced for the hash-sum-mismatch race between mirror sync and client download; supported from APT 1.2.0 [3p, packagecloud]. https://wiki.debian.org/DebianRepository/Format , https://blog.packagecloud.io/eng/2016/09/27/fixing-apt-hash-sum-mismatch-consistent-apt-repositories/ (redirect target; page not fetched)
- Pdiffs: `$I.diff/Index` with patch history; clients apply patches in sequence; rred merge bugs and slowness reports exist (2006-2011 Debian bug threads; "Acquire::PDiffs=false" the standard workaround) [3p, mailing lists]. No current measurement found.
- Compression: "Clients must support xz"; servers "should offer only xz", gzip kept for historical clients.
- Signed `InRelease` with `Date`/`Valid-Until` bounds replay.

**winget (pre-indexed source)** [doc, 3p]
- Default source `https://cdn.winget.microsoft.com/cache`, type `Microsoft.PreIndexed.Package` (signed `source.msix` bundling a SQLite `index.db`), alternative type `Microsoft.Rest`; `winget source update` forces refresh; source has an `Updated` timestamp. Whole-catalog download model with a REST alternative; ETag/size/interval not found in docs. https://learn.microsoft.com/en-us/windows/package-manager/winget/source

### C. Static hosting: ETag and compression

**GitHub Pages**
- Official limits doc: site <= 1 GB, soft 100 GB/month bandwidth, soft 10 builds/hour (not applied to custom Actions workflows), 10-minute deploy timeout; says nothing about caching headers, ETags or compression. https://docs.github.com/en/pages/getting-started-with-github-pages/github-pages-limits
- Single-author measurement [3p] on a GitHub Pages site (2026-08-03): ETag `"<hex>-<hex>"` decoded as mtime + size; eight URLs including a year-old post all returned the same `Last-Modified` (deploy stamp); with `Accept-Encoding: gzip` the response gained `content-encoding: gzip` and a weak `W/"..."` ETag derived from the uncompressed size. The author's identical-bytes redeploy test ran on a local server, not on Pages, so "ETag changes across redeploys of identical bytes on GitHub Pages" is inferred from the deploy-stamped mtime, not observed. https://dev.to/jangwook_kim_e31e7291ad98/your-last-deploy-reset-every-cache-validator-on-the-site-4dd3
- `Cache-Control: max-age=600` reported by several third-party posts; GitHub does not document it and custom headers are reported unsupported [3p].
- Compression of JSON on Pages: no documentation found. HTML gzip observed [3p, above]. Brotli: no source found.
- PEP 691 states GitHub Pages "doesn't allow custom types" (content negotiation / custom media types), relevant to Accept-based layout switching on Pages.

**GitLab Pages**
- Docs: Pages checks for sibling `.br` or `.gz` and serves it when the client accepts; precompression covers extensions **htm, html, xml, txt, text, js, css, svg** (JSON is not in the list); both copies must be in the artifact; Pages "doesn't compress on demand". https://docs.gitlab.com/user/project/pages/introduction/ (compressed-assets section)
- Brotli takes priority over gzip when both accepted (MR !359); gzip MR !25 requires the uncompressed file to stay. https://gitlab.com/gitlab-org/gitlab-pages/-/merge_requests/25 , https://gitlab.com/gitlab-org/gitlab-pages/-/merge_requests/359
- Cache headers: issue reports `max-age=600` and asks for revalidation (`must-revalidate` or `Last-Modified`) https://gitlab.com/gitlab-org/gitlab-pages/-/issues/711 ; ETag support for zip serving is a separate MR (!588) whose description gives no mechanism. How ETag/Last-Modified are produced for identical bytes across redeploys: not documented.

## negative:

- Claude Code and Copilot marketplace docs state no catalog entry-count limit, no `marketplace.json` size limit for git mode, and no delta/ETag mechanism. The only hard numbers are the 5 MiB / 10 s URL-mode cap and the 120 s clone timeout. No public report of a marketplace at 10k+ entries found.
- No source documents GitHub Pages or GitLab Pages emitting a content-derived ETag. Evidence points to mtime+size style tags, but the redeploy-with-identical-bytes case was not measured on either host.
- GitHub Pages JSON/brotli behaviour: undocumented. GitLab Pages precompressed list excludes `json`.
- Open VSX API reference and VS Code gallery rate limits could not be retrieved as text; JetBrains Marketplace IDE-update request format not found.
- Homebrew posts, Cargo official posts and PEP 691 give no measured bandwidth or latency numbers; the only figures are third-party anecdotes (Cargo CI timings) or Cargo RFC size estimates.
- Helm scaling figures are secondary (issue trackers via search); no Helm project statement on an index size limit.
- No dual-serving duration found for MCP Registry v0 to v0.1, Homebrew git taps (post-4.0 tap clones still optional via opt-out, no end date), or Cargo (indefinite by stated policy).
- APT pdiff current-day measurements: none found; only 2006-2014 bug reports.
- Go sumdb tile cache lifetimes and TUF migration outcomes: not found.

## leads:

- Cargo sparse RFC full text and `cargo/issues/10965` for the sparse design rationale and stale-CDN handling: https://github.com/rust-lang/cargo/issues/10965
- Homebrew: `Library/Homebrew/api/internal/` and the PR introducing JWS signing; `formulae.brew.sh` repo for how the JSON is generated and hosted: https://github.com/Homebrew/formulae.brew.sh
- npm replication migration thread (full text, later months) for post-cutover failure modes: https://github.com/orgs/community/discussions/152515
- Go checksum database design (tlog tiles, caching): https://research.swtch.com/tlog
- `curl -sI -H 'Accept-Encoding: br, gzip'` against a throwaway GitHub Pages and GitLab Pages project, two deploys of identical bytes, comparing `etag`, `last-modified`, `content-encoding`, `cache-control` for a `.json` file: would settle section C directly. (Bash network access was denied in this session, so it was not run.)
- Helm chart repository index scaling: fluxcd/source-controller#303, fluxcd/helm-controller#1264, gitlab-org/gitlab#579785 (each cited via search results only).
- Debian: APT `apt-pkg/acquire-item.cc` pdiff handling and Debian wiki `DebianRepository/Format` hash-index section; Colin Watson's "No more Hash Sum Mismatch errors".
- winget-cli source for `Microsoft.PreIndexed.Package` update logic (ETag/size) and the REST source spec: https://github.com/microsoft/winget-cli
- PyPI Warehouse: when JSON Simple API went live and any HTML/JSON byte-size comparison; uv/pip request headers for conditional fetch of `/simple/<name>/`.
- MCP Registry GitHub repo changelog/discussions for the v0 to v0.1 transition and any `ETag` support: https://github.com/modelcontextprotocol/registry
