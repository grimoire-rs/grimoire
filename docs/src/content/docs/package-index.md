---
title: "The Package Index"
description: "Most OCI registries cannot answer what packages exist — the package index is Grimoire's answer to that question."
---
<!-- doc_type: reference -->

Most OCI registries cannot answer the question *"what packages exist?"*
The `_catalog` endpoint that grim's browse surfaces (`search`, the TUI,
MCP) rely on is gated or absent on [GHCR][ghcr], [GitLab SaaS][gitlab-reg],
and [Docker Hub][dockerhub]. A **package index** fills that gap: a small,
decentralized directory of package pointers that grim reads instead of
`_catalog`.

## No Central Service {#decentralized}

Grimoire is decentralized by design. Anyone can host an index (a git
repository or a folder of static files), and any OCI registry can host
the packages it points to. The happy path is the default index at
[index.grimoire.rs][index-site], maintained at
[grimoire-rs/index][index-repo] on GitHub — but nothing in grim is
hard-wired to it, and standing up your own is
[one command](./hosting-an-index.md).

> **Phone book, not catalog.** The index stores *pointers* — name, kind,
> OCI ref, description, ownership. It never stores versions. grim
> resolves tags live from the registry at install time, so a stale index
> can never serve a stale version.

## Consuming an Index {#consuming}

A [`[[registries]]`](./configuration.md) entry declares **exactly one**
of `url` / `index`:

```toml
# grimoire.toml
[[registries]]
alias = "hub"
index = "https://index.grimoire.rs"   # package index (browse source)
default = true

[[registries]]
alias = "corp"
oci = "registry.corp.example/team"    # plain OCI registry (_catalog)
```

`oci` and `index` are mutually exclusive because they answer the same
question differently: an `oci` entry lists what *that registry* holds via
`_catalog`; an `index` entry lists whatever the index points to — its
entries carry their own fully-qualified registry refs and may span many
registries.

Two transports, chosen by the locator's shape:

| Locator shape | Transport |
|---|---|
| `http://…`, `https://…` | Static files. grim reads the [index root][consuming-discovery] and the blob it names, or `<base>/all.json` when there is no usable root |
| `git+…`, `ssh://…`, `git@…`, or ending in `.git` | Git. grim shallow-clones and walks `index/**/metadata.json` ([how the clone runs][consuming-git]) |

Both transports share the regular catalog machinery: the per-source
cache under `$GRIM_HOME/catalog/`, the 1-hour TTL, `--refresh`, and
offline degradation (`--offline` serves the cached listing and never
touches the network). A refresh that fails serves the cached listing too,
with a warning naming when it was built. That covers an unreachable host, a
`5xx`, and an oversized or unparsable body. With no cache, the source reads
as an empty group. The failure also starts a [backoff][consuming-backoff], so
a dead host is not asked again on every command.

A cached listing that an earlier grim capped at 500 entries is rebuilt on
the next command instead of served until its TTL expires. If that rebuild
fails, grim serves the capped rows with the incomplete-listing warning
(`grim search --refresh` forces the rebuild).

The static transport sends `Accept-Encoding: gzip` on every package-index
request: the root, `all.json`, `stats.json` and the `c/f` blobs. No other
client of grim sends it, and nothing is required of the host. A host that
serves `all.json` or `stats.json` compressed saves transfer, and grim decodes
the gzip body. A `c/f` blob is already a gzip file, so grim inflates it and
also accepts a blob that a host or CDN decoded in transit.

grim decodes only `gzip`. Any other `Content-Encoding`, such as `br` or
`deflate`, is refused, and what follows depends on the response:

| Response | Result |
|---|---|
| The root, or a `c/f` blob for the listing | [Counts a fallback][consuming-fallback]: grim reads `all.json` for this refresh |
| `all.json` | A failed fetch: grim warns, serves the cached listing and starts a [backoff][consuming-backoff] |
| `stats.json`, or the `c/f` blob for the stats | Skipped: the refresh succeeds and the cached ratings stay |

A decoded body is capped at 128 MiB, and an index at 100,000 entries. Past
either limit the fetch fails, so a listing is never silently truncated, and
grim warns naming the source. A `c/f` blob has the same 128 MiB cap, both as
downloaded and once inflated. The root is capped at 64 KiB. The connect step
waits 10 seconds, a transfer that delivers no bytes for 30 seconds is
abandoned, and 300 seconds is the ceiling for any one request.

One refresh of a source, all of its requests together, has a 360 second
deadline. When it runs out, grim cancels the request in flight and treats the
refresh as a transport failure.

CLI equivalent of the config above:

```console
$ grim config registry add hub --index https://index.grimoire.rs --default
$ grim config registry add corp --oci registry.corp.example/team
```

### Discovery {#consuming-discovery}

On each refresh grim looks for an **index root**, a small JSON file that names
the catalog by digest. It derives the root's address from the locator, using
the directory that holds `all.json`:

| Locator | Root grim asks for | `stats.json` |
|---|---|---|
| `https://h/idx` or `https://h/idx/` | `https://h/idx/c/root.json` | `https://h/idx/stats.json` |
| `https://h/idx/all.json` | `https://h/idx/c/root.json` | `https://h/idx/stats.json` |
| `https://h` or `https://h/` | `https://h/c/root.json` | `https://h/stats.json` |
| Any other `.json` file name | None | The sibling `stats.json` |
| A query or a fragment | None | None |
| A git locator | None | None |

The `all.json` address is derived exactly as before, for every locator. A
locator that gets no probe is read the way it always was.

The probe is a single request. It waits 10 seconds at most, accepts 64 KiB at
most and runs inside the refresh deadline. An index with no root costs one
extra small request per refresh and nothing else. A missing root is not
remembered, so the next refresh asks again. The table shows how grim reads each
answer:

| Answer | What grim does |
|---|---|
| `200` with a valid root that names a flat blob | Reads the catalog from the blob, only when the blob's digest is not the one it already holds |
| `200` with a valid root whose `flat` is `null` | Reads `all.json` and drops any root it had adopted. The index chose a flat layout |
| `404` or `410` | Reads `all.json` and drops any root it had adopted |
| `200` that is not a valid root (a single-page-app fallback page, not JSON, a different `schema`, over 64 KiB), a `304` (grim sends no condition), or any other `4xx` (a `403` from object storage for a missing key, a `401`, a `451`) | Reads `all.json` for this refresh and counts a fallback |
| `5xx`, `408`, `429`, a timeout, a connection error or a refused redirect | Fails the refresh when grim has already adopted a root for this source. Otherwise reads `all.json` |

After a root was adopted, an outage is treated as site-wide. grim then skips
`all.json` and serves its cache. Without an adopted root, falling back means
a failed probe cannot break a flat index that already works.

The fallback count is kept in the [sync sidecar][data-layout]. A rising count
shows an index that answers the probe with something that is not a root,
without a warning on every refresh. A valid root or a `404` resets it.
[`grim context`][commands-context] reports it.

### The Flat Blob and the Fallback {#consuming-fallback}

When the root is valid and its `flat` reference differs from the catalog grim
already holds, grim fetches the blob at `<index directory>/c/f/sha256/<digest>.json.gz`.
It inflates the body, stopping once the output passes the size the root
declared. It then checks the SHA-256 and the size against the root. Only then
does it parse the document, exactly as it would parse `all.json`, and adopt
the root. A root whose blob grim already holds costs the root request alone.

A blob that is missing (`404`) or fails verification does not break the
refresh. Then grim does not adopt that root, reads `all.json` instead and
counts a fallback. A transport failure while fetching the blob fails the
refresh. Then grim serves its cache, and the next refresh asks for the root and
the blob again.

grim sends no conditional request. On each refresh past the cache lifetime it
fetches the small root with a plain `GET`, and `--refresh` does the same.
An `ETag` or `Last-Modified` header the host serves is neither stored nor
sent back.

A `200` root whose `flat` reference matches the catalog grim holds downloads no
blob and leaves the cached listing untouched. It restamps the time of the last
check in the sidecar.

### The Stats Rule {#consuming-stats}

A root names its stats blob, a gzip file that holds the `stats.json` bytes.
grim fetches that blob once per advertised digest, with no timer. It fetches
only when the digest differs from the one it applied last and from the one it
rejected last. A refresh that finds the digest already applied makes no stats
request, an unchanged root included.

A blob that is missing, fails verification, does not parse or declares a
`schema_version` above the one grim knows is skipped. The ratings grim has stay
in place. Nothing asks for that digest again until the root names another one.
A transport failure is retried on the next refresh. A root whose `stats` is
`null` advertises nothing, so grim makes no request and keeps what it has.

`--refresh` fetches the blob regardless, and so does `grim rate` for a row
that reads as unrated. When a new blob changes a rating or a download count,
grim rewrites the cached listing once. Otherwise the refresh touches only the
sidecar. The `all.json` path keeps its own rule: grim fetches `stats.json` on
every refresh, and a `404` means unrated.

### Failures and Backoff {#consuming-backoff}

A refresh that cannot reach its source serves the cached listing and records
the failure. Later refreshes then wait. grim makes no request to that source
until the recorded time, so a dead host is asked once per interval.

The interval starts near one minute and doubles with each consecutive failure:
about 1, 2, 4, 8, 16 and 32 minutes. It carries 20 percent of jitter either
way and stops growing at one hour. A `Retry-After` header on a `429` or `503`
can lengthen the wait, never past one hour.

While a source waits, `grim search` makes no request. It serves the cache, and
that source reports `ok: false` in [`sources`][json-sources], with an `error`
that contains `refresh deferred until <timestamp>` and the redacted last
error. A source with no cache reads as an empty group for the same time.

`--refresh` ignores the wait. A success clears it, and a clock set back makes
it expire. `--offline` never makes a request. Index and OCI sources share the
rule.

### Redirects and Credentials {#consuming-redirects}

grim follows up to five redirects on an index request and refuses one from
`https` to `http`. A request carries no cookie and no credential of grim's
own. A user name and password written in the locator are sent as Basic
authentication. They reach only the locator's own host, and a cross-origin
redirect drops them.

They are never written to the sync sidecar or to a log. Every error grim prints
also strips them, and any query or fragment.

### The Git Clone {#consuming-git}

A git locator is cloned with `git clone --depth 1 --no-local`. Only the
`file`, `http`, `https`, `ssh` and `git` transports are allowed, so an `ext::`
URL fails. Git follows HTTP redirects on the first request only. Nothing ever
prompts for a credential, so a private repository needs an ambient credential
helper or an ssh agent.

A transfer slower than 1 KB per second for 30 seconds is abandoned. A clone is
killed after 300 seconds at most, sooner when the refresh deadline is nearer.
A URL in an error loses its credentials, query and fragment first.

## Index Specification (v1) {#spec}

This section is normative for index producers and consumers.

### Repository Layout {#spec-layout}

```
index/
  <host>/<namespace>/              # host = forge instance, namespace = identity there
    <package>/
      metadata.json                # one pointer per package
scripts/                           # (optional) build/validation tooling
```

- `<host>` is the forge instance namespaces are anchored on —
  `github.com` for the default index, a GitLab or GitHub Enterprise host
  for self-hosted indexes.
- `<namespace>` is an account or group name on `<host>`, lowercase as
  registered. On GitLab it is the **full** group path and may span
  multiple segments (`platform/ai`).
- `<package>` is the package name and MUST equal the `name` field in the
  contained `metadata.json`.
- A package name is **claimed by one kind per namespace**. The kind lives
  *inside* `metadata.json`, not in the path, so a skill and a bundle both
  named `hex` would resolve to one pointer and the second would overwrite
  the first. `grim publish --announce` refuses a manifest whose entries
  share a name before it pushes anything (exit 65) — publishing the same
  two names *without* `--announce` stays fine, since they occupy distinct
  OCI repositories, and so does marking all but one of them
  `announce = false` in the manifest.
- Top-level directories that are not a host are *reserved* (vanity
  namespaces; maintainer-approved on the default index).

### `metadata.json` {#spec-metadata}

```json
{
  "schema": 1,
  "name": "grim-usage",
  "kind": "skill",
  "ref": "ghcr.io/grimoire-rs/skills/grim-usage",
  "description": "Drive the grim CLI — install, update, search, publish.",
  "summary": "Drive the grim CLI from an agent.",
  "keywords": ["grim", "install", "search", "publish"],
  "repository": "https://github.com/grimoire-rs/grimoire",
  "owner": { "github": "grimoire-rs", "id": 298895348 }
}
```

| Field | Type | Required | Constraints |
|---|---|---|---|
| `schema` | integer | yes | Metadata schema version. This document specifies `1`. Consumers MUST skip entries with an unknown `schema` (forward compatibility). |
| `name` | string | yes | Package name. MUST equal the directory name containing the file. |
| `kind` | string | yes | One of `skill`, `rule`, `agent`, `mcp`, `bundle`, `hook` (experimental — see [Hooks](./hooks.md)). |
| `ref` | string | yes | Fully-qualified OCI reference **without a tag**: `registry-host[/namespace]/repository`. MUST contain at least one `/`. MUST NOT carry a tag or digest — versions are resolved live. |
| `description` | string | yes | One line, shown by `grim search` and the TUI. |
| `summary` | string | no | Short single-line blurb, matched by `grim search` alongside `description` (distinct from it — usually shorter). |
| `keywords` | array of string | no | Publisher keywords, matched by `grim search`. Omitted when empty; a `null` reads as empty. |
| `repository` | string | no | Source repository URL. Consumers keep it only with an `https://` prefix. |
| `deprecated` | string | no | Publisher deprecation notice, mirroring the artifact's `com.grimoire.deprecated` annotation. A non-empty value marks the package deprecated — `grim search` and the TUI hide it unless it is installed or `show_deprecated` is on, and mark it when shown. Absent or whitespace-only ⇒ not deprecated. Written here because the pointer is the only source a browse reads: resolving the annotation would cost one manifest fetch per index entry. |
| `replaced_by` | string | no | Successor grim reference, mirroring `com.grimoire.replaced-by`. Independent of `deprecated`. |
| `license` | string | no | SPDX license expression, mirroring `org.opencontainers.image.licenses`. Written here for the same reason as `deprecated`: the pointer is the only source a browse reads. |
| `created` | string | no | Publishing commit date (RFC3339), mirroring `org.opencontainers.image.created` — the browse-time recency signal. Absent for an artifact published without [build provenance](./publishing.md#git-provenance). |
| `owner.github` | string | yes* | GitHub login owning the namespace — for pointers under `index/github.com/` (and other GitHub-forge hosts). MUST match the namespace directory (case-insensitive). |
| `owner.login` | string | yes* | The generic owner key for any non-GitHub host (the pointer's `index/<host>/` segment carries the forge context). MUST match the namespace directory (case-insensitive). |
| `owner.id` | integer | yes | The account's numeric ID on the pointer's host — the GitHub account ID; on GitLab the *group* ID for group namespaces or the *user* ID for user namespaces (user namespace IDs are visible only to their owner, so the public user ID is the one an index validator can verify). Immutable — logins can be deleted and re-registered by someone else; the ID cannot. Validation compares it against the live API. |

\* exactly one of `owner.github` / `owner.login`, matching the pointer's
host. grim's read side ignores `owner` entirely — the fields exist for
the index's own server-side validation.

Unknown additional fields MUST be tolerated by consumers (additive
schema evolution without a version bump).

A consumer MUST NOT let one malformed entry blank the index. grim skips an
`all.json` element whose fields have the wrong type, warns once with the
count, and lists the rest. A document where every element is malformed
fails, so the previous catalog stays.

### Compiled Artifacts {#spec-compiled}

A statically-served index publishes the compiled form:

| Path | Content |
|---|---|
| `/c/root.json` | The [index root](#spec-root): mutable, about half a kilobyte, names the catalog by digest. |
| `/c/f/sha256/<hex>.json.gz` | The [flat and stats blobs](#spec-blobs): immutable gzip files named by the digest of their inflated bytes. |
| `/all.json` | Every package, one JSON array. Each element is the `metadata.json` object plus a derived `namespace` field (e.g. `"github.com/grimoire-rs"`). |
| `/index/<namespace…>/<package>/metadata.json` | Path-addressable copy of each pointer. |
| `/stats.json` | *Optional.* Per-package publisher statistics — [artifact ratings](./ratings.md) and download counts. Absent on an index that publishes none; see [the stats sidecar](#spec-stats). |

grim's HTTP transport needs one of two things: `all.json`, or the root and
the flat blob it names. An index serves `/all.json` and `/stats.json` while
its `legacy` setting is true, which is the default. With `legacy` set to
false it serves only `c/`, and a grim that predates the root can no longer
read it. The path-addressable copies allow cheap single-package lookups by
any consumer without downloading the full set.

The git transport skips compilation entirely: grim walks the
`index/**/metadata.json` tree of the clone, so a plain git repository
with the layout above *is already a fully functional index*. It has no
`stats.json`, so a git-transport index is always unrated and uncounted.

### The Index Root (`c/root.json`) {#spec-root}

The root is the one mutable file a client needs. It names the catalog by
content, so a client learns from half a kilobyte whether anything changed. It
sits at `<index directory>/c/root.json` and is capped at 64 KiB. It is UTF-8
JSON with keys in byte order, no extra whitespace and a trailing newline.

```json
{
  "flat": { "sha256": "3781af9adf3b8eaa8d9de77b5487f07d6afe1aba47fdc2f967dbf9fda177ea4a", "size": 41400000 },
  "generated_at": "2026-10-21T08:00:00Z",
  "generation": 1792560000,
  "planes": {
    "catalog": null,
    "enrich": { "sha256": "38e867c580171031fc413211ad92b896803c73dea036de8c594682161588f6c9", "size": 66000 }
  },
  "schema": 1,
  "search": { "sha256": "d833f7c1426e5170b31eb5d84e9b4af67e9917af3df26bda2f2ca66149818c42", "size": 21000000 },
  "stats": { "sha256": "0e33fde480b700b0bcddb2ee402d19f4c43878283a12bed2769ecef0fa396fb4", "size": 3000000 }
}
```

| Field | Type | Constraints |
|---|---|---|
| `schema` | integer | Always present. Accepted only when exactly `1`. Any other value reads as not a root, and grim falls back to `all.json`. |
| `generation` | integer | Always present, from 0 to 2^53 − 1. A build counter, normally the build's UNIX time and never below the previous root's generation plus one. grim reports it and never compares it. |
| `generated_at` | string | Always present. RFC3339 UTC. |
| `flat` | blob reference or `null` | Always present. Names the flat blob of the same build, or is `null` under `layout: flat`. A `null` makes grim read `all.json`. |
| `stats` | blob reference or `null` | Always present. Names the stats blob of the same build, or is `null` when the build has no `stats.json`. A `null` advertises nothing, and grim keeps the ratings it has. |
| `search` | blob reference or `null` | Always present. The site's search file. grim never reads it. |
| `planes` | object | Always present. `catalog` is reserved and always `null`. `enrich` is a blob reference or `null` and is private to the index pipeline. grim ignores both. |

A **blob reference** is `{ "sha256": <digest>, "size": <integer> }`. A digest
matches `^[0-9a-f]{64}$`, lowercase only, and grim checks it before it builds
any URL. The `size` is greater than 0 and not above the cap for its kind. For
a `.gz` file it is the inflated size.

The field `expires` is reserved. A producer never emits it, and no client
enforces it. Clients ignore unknown fields, and a producer adds fields only
additively under `schema: 1`.

A producer writes the root last, after every file it names exists and
verifies. A host that cannot deploy atomically uploads the blobs before the
root.

### The Flat and Stats Blobs (`c/f/`) {#spec-blobs}

A blob lives at `<index directory>/c/f/sha256/<hex>.json.gz`. The **flat
blob** is the gzip of the exact `all.json` bytes, and an index that sets
`layout: flat` publishes none. The **stats blob** is the gzip of the exact
`stats.json` bytes and exists in every layout when the build has a
`stats.json`. Neither has a raw copy beside it.

The `<hex>` in the name is the SHA-256 of the inflated bytes, and the root's
`size` is the inflated size. A flat blob therefore has the same digest as
`/all.json`. Each blob is capped at 128 MiB inflated, and the downloaded body
has the same cap. A blob is immutable and never revalidated, because its name
changes with its bytes.

A client reads a blob in four steps:

1. Fetch the file as a plain file. If the response carries `Content-Encoding: gzip`, decode that first.
2. If the body starts with the gzip magic `1f 8b`, inflate it. Abort once the output passes the size in the root or 128 MiB. Otherwise a host or CDN already decoded it, and the body is used as it is.
3. Check the SHA-256 and the size against the root.
4. Parse the result exactly as the legacy file of the same name.

An aborted inflate, a bad stream and a mismatch all make the blob failed.

A producer writes the gzip deterministically: one member, a modification time
of 0, no name or comment, and a fixed level. Two builds from the same input
then give identical blobs. The bytes of `all.json` and `stats.json` are not
re-serialized for the blob.

### The Stats Sidecar (`stats.json`) {#spec-stats}

An index that collects [artifact ratings](./ratings.md) or download counts
serves a second compiled document at `<base>/stats.json`, a sibling of
`all.json`. It is a **bag of statistics per artifact ref**, not a ratings
file: `rating` and `downloads` sit side by side inside it and a later signal
lands beside them additively, without a second endpoint and without a
version bump for consumers that ignore it.

```jsonc
{
  // Monotonic integer, crates.io style — NOT semver.
  "schema_version": 1,

  // RFC3339 UTC. Makes staleness observable to every consumer.
  "generated_at": "2026-08-18T09:00:00Z",

  // Per-STAT provider block. Each statistic names its own producer,
  // because they are genuinely different: ratings come from a forge,
  // download counts from the registry. A single top-level `provider`
  // would have been wrong the moment the second signal landed.
  "providers": {
    // Which write mutation `grim rate` issues. A plain string, NOT a
    // tagged union: `target` and `url` are hoisted onto the entry, so this
    // carries no read-path data and an unrecognised value degrades to
    // "readable, not writable".
    "rating": "github",

    // Where those threads live. Host authority only — no scheme, no path,
    // port significant. Omit it on a SaaS index; declare it and every
    // consumer of a GHES or self-managed instance votes against the right
    // host with no per-machine configuration.
    "rating_host": "api.github.com",

    // Which backend the download counts were read from. Absent on an index
    // that publishes none — which is most of them: no OCI distribution-spec
    // endpoint exposes a per-artifact download counter, and neither GHCR
    // nor the GitLab registry offers a vendor one.
    "downloads": "artifactory"
  },

  // Keyed by artifact ref, exactly as it appears in all.json's `ref`.
  // Each ref maps to a bag of SIGNALS. A stat with nothing to say is
  // OMITTED; a ref with no stats at all is omitted; the whole `entries`
  // key may be absent. Absent is first-class at all four levels.
  "entries": {
    "ghcr.io/grimoire-rs/grim-usage": {
      "rating": {
        "up": 42,
        // Opaque. No client parses or constructs it. The forge node id the
        // vote mutation targets.
        "target": "D_kwDOABCDEF4AQtBz",
        // Opaque. The human-facing thread link.
        "url": "https://github.com/grimoire-rs/index/discussions/117"
      },
      "downloads": {
        // Every pull the producer could attribute to the artifact. NOT the
        // sum of `versions`: a channel tag (`canary`) carries real traffic
        // that names no release, so it counts here and appears in no
        // per-release entry. Never derive one from the other.
        "total": 1416,
        // Optional. Per-release counts, keyed by the release tag exactly as
        // `tags` spells it. A floating tag (`latest`, `1.35`) is absent by
        // design — it aliases a release whose count is already here, and a
        // number of its own would read as a second, separate figure.
        "versions": { "1.35.2": 900, "1.35.1": 516 },
        // Optional. When the producer READ the counters, RFC3339 UTC.
        // Distinct from the document's `generated_at`: a run that only
        // recomputes ratings republishes a carried-forward `downloads`
        // whose stamp is older than the document around it, which is what
        // makes displaying the count honest. Treat it as optional — the
        // producer derives it from its own run clock, so any backend can
        // supply it, but none is obliged to.
        "as_of": "2026-08-18T09:00:00Z"
      }
    }
  }
}
```

| Field | Type | Required | Constraints |
|---|---|---|---|
| `schema_version` | integer | yes | Monotonic, **not** semver. This document specifies `1`. |
| `generated_at` | string | yes | RFC3339 UTC timestamp of the run that produced the document. |
| `providers` | object | no | One key per statistic, naming its producer. `providers.rating` is `"github"` or `"gitlab"` — the forge `grim rate` votes through. Absent, or an unrecognised value, leaves every rating **readable but not votable**. |
| `providers.rating_host` | string | no | The forge host those threads live on: host authority only (`host` or `host:port`), no scheme and no path. Absent ⇒ the consumer's built-in per-provider default (`api.github.com` / `gitlab.com`). A value that is not a bare host is ignored and the default applies; a **remote** index declaring one of the loopback forms (`localhost`, `127.0.0.1`, `[::1]`) is ignored too. Against a host declared here, `grim rate` requires `--token-host` for a `--token-stdin` or `GRIM_RATE_TOKEN` credential — see [Voting against a private instance](./ratings.md#voting-host). |
| `entries` | object | no | Keyed by artifact ref, spelled exactly as `all.json` spells it. Absent means nothing is rated yet. |
| `entries[ref].rating` | object | no | `{up, target, url}`. `up` is a non-negative integer count; `target` and `url` are **opaque** — never parsed or constructed by any client. A ref with zero votes is omitted rather than carrying `"up": 0`. |

The consumer contract is the [OSV][osv] formulation, which is the clearest
in the ecosystem:

> A client that understands `schema_version` *N* must accept and correctly
> process any document declaring `schema_version` ≤ *N*, ignoring fields it
> does not recognize. A document declaring `schema_version` > *N* may be
> skipped or degraded to "no rating available", but must never be treated as
> a parse error. A missing file, a missing entry, and a missing field all mean
> **unrated** — never an error, at any of the three levels.

grim implements exactly that: a `404`, a transport or TLS failure, a body
that does not parse, and a `schema_version` above the one it knows all
degrade to *no ratings* at `debug` level, and only the `all.json` fetch
decides whether the catalog build succeeded.

On an index with a root, the root's `stats` reference names a gzip blob of the
same document, and grim applies it once per advertised digest. See [the stats
rule][consuming-stats]. The document, its path and its schema do not change,
and a failed stats blob never fails the refresh either.

### Namespaces and Ownership {#spec-namespaces}

Namespaces are GitHub identities. There is no reservation step: the
first accepted pull request under `index/github.com/<login>/` creates
the namespace. A namespace can only be modified by:

- pull requests authored by `<login>`, or
- pull requests authored by a **public member** of the `<login>`
  organization.

### Validation and Auto-Merge {#spec-validation}

The default index auto-merges announcement PRs when **all** of the
following hold (anything else falls to manual maintainer review). A
GitLab fork applies the same gate with GitLab identities — namespace
ownership becomes group membership; see
[Self-Hosted GitLab Setup](./self-hosted-gitlab.md).

1. Only `index/github.com/<ns>/<pkg>/metadata.json` paths changed.
2. `<ns>` is the PR author's login, or an org the author publicly
   belongs to.
3. `owner.github` matches `<ns>` and `owner.id` matches the account's
   numeric GitHub ID (live API lookup — spoof-proof against login
   recycling).
4. Every changed file passes the schema above.
5. `ref` is *reachable*: the registry lists at least one tag
   anonymously. Publish before you announce.

Deletions inside your own namespace pass the same ownership check and
auto-merge too.

## Announcing Packages {#announcing}

Publish first (the packages must be pullable), then announce:

```console
$ grim publish --announce
```

`--announce` writes/updates your `metadata.json` pointers in a clone of
the index repository, pushes a deterministic topic branch, and opens the
pull/merge request through the forge's REST API — GitHub and GitLab,
enterprise instances included, no `gh`/`glab` CLI needed. A GitLab host
without an API token gets the MR via [git push options][push-options]
instead; a plain git host is left with the pushed branch.

Not every published entry needs a pointer. A manifest entry carrying
`announce = false` publishes as usual but is left out of the announce —
the shape for a bundle member that should be installed through its bundle
rather than found on its own (the bundle's pointer is what the index
carries; the member resolves from the bundle manifest). The opt-out only
ever withholds a pointer: one an earlier run announced stays until a
pull request removes it. See
[the publish.toml format](./publishing.md#batch-publish-manifest).

Most contributors to a public index have no push access to it — that is
the normal case for [grimoire-rs/index][index-repo] itself. Rather than
fail with a permission error, `--announce` detects the missing access (a
GET against the repository or project, when a token is present) and
automatically forks the index into your account — reusing an existing
fork if you already have one, even one renamed since it was created, on
both GitHub and GitLab. On GitLab the reuse is identity-based: grim
enumerates the upstream's forks and matches one owned by the
authenticated user, scoped to your **personal namespace** — a fork later
moved into a group namespace is not reused (tracked follow-up). Because
that listing can briefly lag behind a fork's creation, grim retries the
enumeration with short backoff for up to about 10 seconds before giving
up, so a fork created moments earlier — by this run or a concurrent one
— is not missed. It pushes the topic branch to that fork and opens the
pull/merge request cross-repository, against the real upstream. grim
verifies the fork is genuinely derived from the upstream repository
before pushing to it, so a same-named repository that happens to sit in
your account is never mistaken for the fork. Detection only applies with
a token present and a GitHub or GitLab forge, so a token-less or
plain-git target behaves exactly as before. This is a different use of
a fork than
[self-hosting a copy of the index](#self-hosting-fork): that fork becomes
its own independent index, while the fork `--announce` creates here exists
only to carry one contribution back to someone else's index.

The `fork` key in `[announce]` chooses when that happens. It takes
`"auto"` (the default — fork only when the credential cannot push),
`"never"` (always push straight at the index, failing the same way it
always has when you lack access), or `"always"` (fork and open the
cross-repository pull/merge request even when you *could* push directly).
`"always"` suits a maintainer who wants every announce to land as a
reviewable PR rather than a direct commit. The older boolean spelling
still parses — `true` means `"auto"`, `false` means `"never"` — so an
existing manifest keeps working unchanged. No policy can conjure a fork
of your own repository: when the authenticated identity owns the index,
grim skips the fork and pushes directly, because a repository cannot be
forked into the namespace it already occupies.

When grim has to create a fork rather than reuse one, readiness works
differently per forge. On GitLab, forking runs an import job: grim polls
the fork's import status with exponential backoff (2 seconds, doubling to
a 30-second cap) up to a 5-minute wall-clock deadline before pushing. A
first `--announce` to a GitLab repository you lack push access to can
pause for a few seconds to a couple of minutes while the import
completes; it is not a hang. On GitHub, fork metadata is ready
immediately — there is no import to wait for — but the fork's git objects
provision asynchronously behind that metadata, so the first push can 404
even though the fork already exists. grim covers this with a single
~3-second retry on that push; if the objects still are not ready by then,
the announce fails (exit 69), the packages are already published, and
re-running `--announce` succeeds once the fork has settled, since the
fork is adopted rather than recreated.

The full `[announce]` surface:

```toml
# publish.toml
registry = "ghcr.io"

[announce]
repository = "https://github.com/grimoire-rs/index"  # default
forge      = "github"            # github | gitlab | plain; default: auto
host       = "github.com"        # index/<host>/ segment; default: derived
api_url    = "https://api.github.com"  # default: CI env / forge convention
namespace  = "your-login"        # full group path on GitLab
owner_id   = 12345               # default: resolved via the forge API
fork       = "always"            # never | auto | always; default: auto
```

Every field except `repository` resolves automatically in the common
cases:

- **`host`** derives from the repository URL (a local-path locator has no
  host — set it explicitly).
- **`forge`** — explicit value > the CI environment **when its server
  host equals the index host** > `github` for github.com > `plain`.
- **`api_url`** — explicit > host-matched CI (`GITHUB_API_URL` /
  `CI_API_V4_URL`) > convention: `api.github.com`, `<host>/api/v3` on
  GitHub Enterprise, `<host>/api/v4` on GitLab.
- **token** — `GRIM_ANNOUNCE_TOKEN` always wins; in a host-matched CI the
  conventional variables apply (`GH_TOKEN`/`GITHUB_TOKEN`,
  `GITLAB_TOKEN` — never `CI_JOB_TOKEN`, it cannot open MRs). Tokens are
  sent as API headers only and never logged. The git **push transport**
  is separate: on a host-matched GitLab CI runner it falls back to
  `gitlab-ci-token:$CI_JOB_TOKEN` when no ambient git credential answers
  — transport only, never the API
  ([details](./ci.md#gitlab-announce-job-token)).
- **`namespace`** — explicit > host-matched CI
  (`GITHUB_REPOSITORY_OWNER` / `CI_PROJECT_NAMESPACE`) > the
  authenticated GitHub API user.
- **`owner_id`** — explicit > forge API lookup (GitHub always; GitLab
  with a token). A plain host requires it explicitly.
- **`fork`** — `auto` (default) forks only when the credential lacks push
  access to `repository`; `never` forces every announce straight at
  `repository`, failing with the same permission error grim raised before
  auto-forking existed; `always` forks and opens the cross-repository
  PR/MR even with push access — it still needs a forge API token, like
  every fork path. The legacy booleans remain accepted (`true` = `auto`,
  `false` = `never`); a self-owned index never forks under any policy.

The host-match gate is deliberate: a GitLab pipeline announcing to a
GitHub index inherits **nothing** from the GitLab CI environment — wire
the cross-forge credential through `GRIM_ANNOUNCE_TOKEN` and set `forge`
explicitly.

> **Migrating from grim ≤ 0.6**: announces to non-GitHub hosts used to
> write pointers under `index/github.com/…` unconditionally. They now
> land under the real `index/<host>/…` — delete the stale
> `index/github.com/` entries from such an index (the reader walks every
> pointer, so leftovers appear as duplicates).

Announcing straight from a pipeline (GitHub Actions or GitLab CI, with
the token wiring each forge needs) is covered in
[Publishing from CI](./ci.md); running the whole index on a corporate
GitLab is covered in [Self-Hosted GitLab Setup](./self-hosted-gitlab.md).

## Hosting Your Own Index {#self-hosting}

One command scaffolds a complete index repository — content tree, site
config, contribution gate, and CI for [GitHub Pages][gh-pages] or
[GitLab Pages][gl-pages]:

```console
$ npx @grimoire-rs/indexer init
$ git push
$ grim config registry add acme --index https://acme.github.io/index
```

[Host Your Own Index](./hosting-an-index.md) is the walkthrough: prompts,
what lands on disk, deploying to either forge, accepting contributions,
branding, and running one privately. The two lower-level shapes below
stay valid — nothing in grim requires the toolchain.

An index repository can also host a plugin marketplace: commit a
`marketplace.toml` beside the package pointers and deploy the export to a
sibling repository. See [Host a plugin
marketplace](./guides/hosting-a-marketplace.md#department).

### A Plain Git Repository {#self-hosting-git}

Simplest — works everywhere, with no build step at all. Create a
repository with the layout above, on GitHub, [GitLab][gitlab], or any git
host. Done. Consumers configure:

```toml
[[registries]]
alias = "team"
index = "https://gitlab.com/your-group/index.git"
```

Private repositories work through ambient git credentials (credential
helper or ssh agent) — grim never prompts.

### Static Files {#self-hosting-static}

Fastest for consumers, and what the scaffolded CI publishes: compile
`all.json` plus the path-addressable pointer copies and the index root, and
serve the `dist/` folder from [GitHub Pages][gh-pages], [GitLab Pages][gl-pages], or any
webserver:

```toml
[[registries]]
alias = "team"
index = "https://index.your-domain.example"
```

`npx @grimoire-rs/indexer build` performs that compilation, and renders
the browsable catalog alongside it. A folder you assemble by hand needs only
`all.json`. The root and the blobs are optional, and
[Host Your Own Index][hosting-sync] covers the `sync` block that controls them.

### Fork an Existing Index {#self-hosting-fork}

Forking [grimoire-rs/index][index-repo] inherits its **entries** as well
as its layout, which is what makes it the right move only when you want
the public catalog as a starting point — a mirror, or an internal index
seeded from the public one. Starting empty is `init`, above.

Either way, this is a repository you run as your own index, long-lived
and configured as a `[[registries]]` target — distinct from the throwaway
fork [`--announce` creates automatically](#announcing) to contribute a
pointer back to someone else's index. For the full corporate GitLab
walkthrough — CI variables, auto-merge by group membership, release
mirrors — see [Self-Hosted GitLab Setup](./self-hosted-gitlab.md).

## Relationship to Registries {#registries}

The index and the registry are independent axes:

| | Default | Self-hosted |
|---|---|---|
| **Packages (OCI)** | `ghcr.io/…` (any public registry) | [Zot][zot], [Harbor][harbor], GitLab registry, … |
| **Discovery (index)** | `index.grimoire.rs` | git repo or static files anywhere |

Mix freely: a public index can point at private registries (consumers
authenticate via [`grim login`](./authentication.md)), and a private
index can point at public packages.

<!-- external -->
[push-options]: https://docs.gitlab.com/topics/git/commit/#push-options
[ghcr]: https://docs.github.com/en/packages/working-with-a-github-packages-registry/working-with-the-container-registry
[gitlab-reg]: https://docs.gitlab.com/ee/user/packages/container_registry/
[dockerhub]: https://hub.docker.com/
[gitlab]: https://gitlab.com/
[gh-pages]: https://pages.github.com/
[gl-pages]: https://docs.gitlab.com/ee/user/project/pages/
[zot]: https://zotregistry.dev/
[harbor]: https://goharbor.io/
[osv]: https://ossf.github.io/osv-schema/

<!-- internal -->
[consuming-discovery]: #consuming-discovery
[consuming-backoff]: #consuming-backoff
[consuming-fallback]: #consuming-fallback
[consuming-git]: #consuming-git
[consuming-stats]: #consuming-stats
[data-layout]: ./configuration.md#data-layout
[commands-context]: ./commands.md#context
[json-sources]: ./json-interface.md#search-sources
[hosting-sync]: ./hosting-an-index.md#sync

<!-- grimoire -->
[index-site]: https://index.grimoire.rs
[index-repo]: https://github.com/grimoire-rs/index
