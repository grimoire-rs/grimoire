# The Package Index

Most OCI registries cannot answer the question *"what packages exist?"*
The `_catalog` endpoint that grim's browse surfaces (`search`, the TUI,
MCP) rely on is gated or absent on [GHCR][ghcr], [GitLab SaaS][gitlab-reg],
and [Docker Hub][dockerhub]. A **package index** fills that gap: a small,
decentralized directory of package pointers that grim reads instead of
`_catalog`.

Grimoire is decentralized by design. Anyone can host an index (a git
repository or a folder of static files), and any OCI registry can host
the packages it points to. The happy path is the default index at
[index.grimoire.rs][index-site], maintained at
[grimoire-rs/index][index-repo] on GitHub — but nothing in grim is
hard-wired to it.

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
| `http://…`, `https://…` | Static files — grim fetches `<base>/all.json` |
| `git+…`, `ssh://…`, `git@…`, or ending in `.git` | Git — grim shallow-clones and walks `index/**/metadata.json` |

Both transports share the regular catalog machinery: the per-source
cache under `$GRIM_HOME/catalog/`, the 1-hour TTL, `--refresh`, and
offline degradation (`--offline` serves the cached listing and never
touches the network).

CLI equivalent of the config above:

```console
$ grim config registry add hub --index https://index.grimoire.rs --default
$ grim config registry add corp --oci registry.corp.example/team
```

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
| `kind` | string | yes | One of `skill`, `rule`, `agent`, `mcp`, `bundle`. |
| `ref` | string | yes | Fully-qualified OCI reference **without a tag**: `registry-host[/namespace]/repository`. MUST contain at least one `/`. MUST NOT carry a tag or digest — versions are resolved live. |
| `description` | string | yes | One line, shown by `grim search` and the TUI. |
| `summary` | string | no | Short single-line blurb, matched by `grim search` alongside `description` (distinct from it — usually shorter). |
| `keywords` | array of string | no | Publisher keywords, matched by `grim search`. Omitted when empty. |
| `repository` | string | no | Source repository URL. Consumers keep it only with an `https://` prefix. |
| `deprecated` | string | no | Publisher deprecation notice, mirroring the artifact's `com.grimoire.deprecated` annotation. A non-empty value marks the package deprecated — `grim search` and the TUI hide it unless it is installed or `show_deprecated` is on, and mark it when shown. Absent or whitespace-only ⇒ not deprecated. Written here because the pointer is the only source a browse reads: resolving the annotation would cost one manifest fetch per index entry. |
| `replaced_by` | string | no | Successor grim reference, mirroring `com.grimoire.replaced-by`. Independent of `deprecated`. |
| `owner.github` | string | yes* | GitHub login owning the namespace — for pointers under `index/github.com/` (and other GitHub-forge hosts). MUST match the namespace directory (case-insensitive). |
| `owner.login` | string | yes* | The generic owner key for any non-GitHub host (the pointer's `index/<host>/` segment carries the forge context). MUST match the namespace directory (case-insensitive). |
| `owner.id` | integer | yes | The account's numeric ID on the pointer's host — the GitHub account ID; on GitLab the *group* ID for group namespaces or the *user* ID for user namespaces (user namespace IDs are visible only to their owner, so the public user ID is the one an index validator can verify). Immutable — logins can be deleted and re-registered by someone else; the ID cannot. Validation compares it against the live API. |

\* exactly one of `owner.github` / `owner.login`, matching the pointer's
host. grim's read side ignores `owner` entirely — the fields exist for
the index's own server-side validation.

Unknown additional fields MUST be tolerated by consumers (additive
schema evolution without a version bump).

### Compiled Artifacts {#spec-compiled}

A statically-served index publishes the compiled form:

| Path | Content |
|---|---|
| `/all.json` | Every package, one JSON array. Each element is the `metadata.json` object plus a derived `namespace` field (e.g. `"github.com/grimoire-rs"`). |
| `/index/<namespace…>/<package>/metadata.json` | Path-addressable copy of each pointer. |

`all.json` is the only endpoint grim's HTTP transport requires. The
path-addressable copies allow cheap single-package lookups by any
consumer without downloading the full set.

The git transport skips compilation entirely: grim walks the
`index/**/metadata.json` tree of the clone, so a plain git repository
with the layout above *is already a fully functional index*.

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
your account is never mistaken for the fork. Set `fork = false` in
`[announce]` to opt out and always push directly, which fails the same
way it always has when you lack access; detection only applies with a
token present and a GitHub or GitLab forge, so a token-less or
plain-git target behaves exactly as before. This is a different use of
a fork than
[self-hosting a copy of the index](#self-hosting-fork): that fork becomes
its own independent index, while the fork `--announce` creates here exists
only to carry one contribution back to someone else's index.

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
fork       = false               # default: true (auto-fork on missing push access)
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
- **`fork`** — default `true` (auto-fork when the credential lacks push
  access to `repository`); `fork = false` forces every announce straight
  at `repository`, failing with the same permission error grim raised
  before auto-forking existed.

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

Any of the following is a complete, working index:

### A Plain Git Repository {#self-hosting-git}

Simplest — works everywhere. Create a repository with the layout above,
on GitHub, [GitLab][gitlab], or any git host. Done. Consumers configure:

```toml
[[registries]]
alias = "team"
index = "https://gitlab.com/your-group/index.git"
```

Private repositories work through ambient git credentials (credential
helper or ssh agent) — grim never prompts.

### Static Files {#self-hosting-static}

Fastest for consumers. Compile `all.json` (see [`scripts/build.py`][build-py]
in the default index for a ~50-line reference) and serve the `dist/`
folder from [GitHub Pages][gh-pages], [GitLab Pages][gl-pages], or any
webserver:

```toml
[[registries]]
alias = "team"
index = "https://index.your-domain.example"
```

### Fork the Default Index {#self-hosting-fork}

Fork [grimoire-rs/index][index-repo] to inherit the layout, the build
script, the Pages deployment, and the validation / auto-merge pipeline
in one step — the repo ships **both** a GitHub Actions workflow and a
`.gitlab-ci.yml`, so a fork works on either forge (the foreign CI files
stay inert). For the full corporate GitLab walkthrough — CI variables,
auto-merge by group membership, release mirrors — see
[Self-Hosted GitLab Setup](./self-hosted-gitlab.md).

This is a fork you run as your own index, long-lived and configured as a
`[[registries]]` target — distinct from the throwaway fork
[`--announce` creates automatically](#announcing) to contribute a pointer
back to someone else's index.

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

<!-- grimoire -->
[index-site]: https://index.grimoire.rs
[index-repo]: https://github.com/grimoire-rs/index
[build-py]: https://github.com/grimoire-rs/index/blob/main/scripts/build.py
