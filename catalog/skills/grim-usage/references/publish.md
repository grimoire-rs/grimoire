# Publishing Workflow

You loaded this file because you are turning a local skill, rule, agent,
or bundle into a published OCI artifact — the build → dry-run → release
loop, version tagging, and registry authentication.

Contents: [Build, Then Release](#build-then-release) ·
[Cascade Tags](#cascade-tags) · [Immutability](#immutability) ·
[Scripted Publishing](#scripted-publishing) ·
[Announcing to the Index](#announce) · [Bundles](#bundles) ·
[Catalog Metadata](#catalog-metadata) ·
[Description Companion](#description-companion) ·
[Authentication](#authentication)

Flags shown here are grim 0.10.x; confirm with `grim <cmd> --help` before
relying on one.

## Build, Then Release

The publishing loop has three steps, each catching mistakes earlier than
the next:

```sh
grim build ./code-review                                        # validate + pack, no push
grim release ./code-review ghcr.io/acme/code-review:1.2.3 --dry-run  # print the push plan
grim release ./code-review ghcr.io/acme/code-review:1.2.3       # validate, pack, push
```

`grim build <path>` detects the kind from the path — a directory packs
as a **skill**, a `.md` file as a **rule**, a `.toml` file as a
**bundle**. An **agent** always needs `--kind agent`: its `.md` shape is
indistinguishable from a rule and grim never guesses from content. This
is the most common publishing mistake — see
[troubleshooting.md](troubleshooting.md).

`--dry-run` prints every tag the release would move and the digest each
would point at, without touching the registry. Make it a habit before
any version release.

## Cascade Tags

Releasing a full semver version moves the floating tags consumers track.
`grim release … :1.2.3` pushes `1.2.3` **and** moves `1.2`, `1`, and
`latest` to the same digest — that is what lets a consumer who declared
`:1` pick up `1.2.3` with a plain `grim update`.

A non-version tag (`canary`, `edge`) publishes only that exact tag, no
cascade. A reference with no tag at all is an error. Two flags make the
cascade explicit: `--cascade` asserts it and rejects a non-semver tag with
exit 65 (a typo guard); `--no-cascade` publishes only the exact tag even for
a full semver.

## Immutability

An exact-version tag is immutable by default: if `1.2.3` already exists
and points at different bytes, the release refuses (exit 65) rather than
rewrite history. Pass `--force` only when you deliberately mean to move
it. Floating tags (`1.2`, `1`, `latest`) move freely on every cascade —
that is their job.

## Scripted Publishing

For multi-package repositories, `grim publish` supersedes the manual
release loop below. It reads a `publish.toml` manifest, validates every
entry before touching the registry, and releases each package in a fixed
kind order — see [Manifest-Driven Batch Publishing](#manifest-driven-batch-publishing).
The manual loop remains useful for edge cases: per-package divergent flags,
or environments where `grim publish` is not available.

Publishing several packages from one repository? Keep their versions in
a reviewed manifest file (versions change only via commits, so the repo
records exactly what was published), then blanket-rerun the release for
every package with `--skip-existing` (conflicts with `--force`):

```sh
grim release ./skills/code-review reg.example.com/skills/code-review:1.3.0 --skip-existing
```

An already-published version is a success no-op — nothing pushes, no
tags move — so only the packages whose version you bumped go out. The
maintenance loop becomes: change content, bump that package's version,
rerun the whole publish. Two rules keep it sound: bump on every content
change (an unbumped change is silently never published), and release
bundle members before the bundle that references them.

## Manifest-Driven Batch Publishing

`grim publish` is the built-in command for multi-package repositories. It
reads a `publish.toml` manifest that declares every package with a `registry`
and per-entry `version`, validates the whole set before any push, then
releases each entry in a fixed kind order: skills first, then rules, then
agents, then MCP servers, then bundles — alphabetical within each kind.
Bundle members always land before the bundles that reference them.

The manifest format uses per-entry sub-tables keyed by name. A minimal
example:

```toml
registry = "ghcr.io"
version = "0.10.0"   # optional catalog-wide version

[skills.code-review]
version = "1.2.0"    # explicit per-entry version wins

[rules.style]         # no version → inherits 0.10.0 (or `version = "${version}"`)

[bundles.dev-stack]
version = "0.3.0"
pin = true          # bundle-only: freeze floating member tags to digests
```

Key behaviors — confirmed invariants, not subject to minor-release drift:

- **Skip-existing by default.** An already-published exact version is a
  success no-op. Only bumped versions push. Use `--force` to move an
  existing exact-version tag instead (the two modes are mutually exclusive).
- **Fail-fast.** The first failing entry stops the batch. The report shows
  all completed entries plus the failed one. Re-run with `--only <name>` to
  resume from a specific entry.
- **`pin = true` is bundle-only.** Setting it on a skill, rule, or agent
  entry is a validation error (exit 65).
- **Catalog-wide version.** `--version` is the single version source for a
  run. An optional top-level `version` covers every entry that omits its
  own or sets the literal `${version}`; explicit per-entry versions
  always win. A **semver** `--version` overrides the top-level value and
  every entry cascades; a **non-semver** `--version` is a movable channel
  tag applied to every entry uniformly, with no cascade. Every version
  input first has the manifest's `version_prefix` (default `v`) stripped,
  so `--version v1.2.3` publishes `1.2.3`; pushed tags stay plain
  `X.Y.Z`. A prerelease or build-metadata value (`1.2.3-rc.1`), a reserved
  cascade-float shape (`latest`, a bare major, or `major.minor`), or a
  value that is not a legal OCI tag all exit 65 at validation, before any
  push. An entry with no version anywhere also exits 65.
- **Namespace overrides.** By default an entry publishes to
  `{registry}/{kind-subdir}/{name}`. A manifest-level `repository_prefix`
  replaces the `{kind-subdir}` segment (`{prefix}/{name}`); a per-entry
  `repository` is used verbatim (name not appended) and wins over the prefix.
  Needed for registries that require a group/project path, e.g. GitLab.
  Full schema and charset rules: [Batch publishing with a manifest][batch-publish].
- **CLI-enforced prefix.** A `--registry` value carrying a path after the
  host (`--registry registry.example/group/project`) splits at the first
  `/`: the host overrides the manifest `registry`, and the rest is an
  enforced namespace prepended to every entry's resolved repository —
  including a verbatim per-entry `repository`. The manifest `registry`
  field itself stays a plain host (a path inside it exits 65). Lets a CI
  pipeline force a publish run under a namespace without editing the
  manifest.
- **Push vs pull registries.** An optional manifest `push_registry`
  (or `--push-registry <host[/prefix]>`, which wins) pushes every entry
  to a different endpoint — a staging registry, an internal push URL
  mirrored to the public name — while the manifest `registry` stays the
  canonical PULL name baked into every reference, annotation (source
  fallback), pinned bundle member id, announce pointer, and report `ref`.
  The JSON report's always-present `pushed_to` field carries the
  push-side reference (`null` when the split is inactive). A malformed
  value exits 65 before any push. `grim release` takes the same
  `--push-registry` flag.

Common flags — confirm current spelling with `grim publish --help`:

```sh
grim publish --dry-run           # plan without pushing
grim publish --only code-review  # publish one entry
grim publish --version canary    # channel tag on every entry, no cascade
grim publish --version 1.4.0 --no-cascade  # single exact tag, no floats moved
grim publish --manifest staging/publish.toml  # alternate manifest
grim publish --announce          # publish, then announce to the index
```

See [Batch publishing with a manifest][batch-publish] for the full schema,
source layout conventions, and disambiguation from bundle TOML files.

## Announcing to the Index {#announce}

Most registries (GHCR included) cannot answer "what packages exist?" —
`grim search` / the TUI / MCP browse a [package index][package-index]
instead. `--announce` is how a publish makes itself discoverable there.

```sh
grim publish --announce                                    # default index repo
grim publish --announce --announce-repo https://github.com/acme/index  # override
```

`--announce` only runs after every planned entry in the batch is fully
published (freshly pushed or already present via skip-existing) and never
under `--dry-run` (prints `announce: skipped (dry run)` instead). It
writes one `index/<host>/<namespace>/<package>/metadata.json` pointer per
published entry into a clone of the index repository, on a deterministic
topic branch, then opens the pull/merge request via the forge REST API
(GitHub or GitLab — enterprise instances included, no `gh`/`glab` CLI).
A GitLab host without an API token gets the MR via git push options; a
plain git host gets the pushed branch and its URL printed.

Configure the target and ownership in an optional `[announce]` table in
`publish.toml`:

```toml
[announce]
repository = "https://github.com/grimoire-rs/index"  # default
forge = "github"                 # github | gitlab | plain; default: auto
host = "github.com"              # index/<host>/ segment; default: derived from repository
api_url = "https://api.github.com"  # default: CI env / forge convention
namespace = "your-login"         # full group path on GitLab
owner_id = 12345678              # default: forge API lookup
```

`--announce-repo` overrides `[announce] repository` for one invocation.
In CI, forge / API URL / token / namespace auto-detect from the standard
environment (`GITHUB_*`, `CI_*`/`GITLAB_TOKEN`) — but **only when the CI
server host equals the index host**; a cross-forge announce wires its
token through `GRIM_ANNOUNCE_TOKEN` (which always wins) and sets `forge`
explicitly. `owner_id` is resolved via the forge API (GitHub always,
GitLab with a token) — set it explicitly for hermetic runs, plain git
hosts, or token-less GitLab. On a host-matched GitLab runner the git
*push* additionally falls back to `gitlab-ci-token:$CI_JOB_TOKEN` when no
other git credential answers (transport only — the job token is never
used for the MR API), so a same-instance announce can run with zero
secrets once the index project's job-token permissions allow the push.
Announce also tolerates GitLab's `HOME`-less step environments.

**Announce failure after a successful publish exits 69** (`Unavailable`)
— the publish report stands; only the index write/PR failed. Announce
*misconfiguration* (missing `host`/`namespace`/`owner_id`) exits 64.

The outcome is machine-readable: `grim publish --format json` emits a
wrapper object `{"items": [...], "announce": ...}` where `announce`
carries `{outcome, branch, url}` — `outcome` is `pull-request`,
`branch-pushed`, or `up-to-date`, and `branch` (the deterministic topic
branch) is always present, so CI reads it from stdout instead of grepping
stderr. `announce` is `null` when the step did not complete (no
`--announce`, dry run, a fail-fast stop, or failure).

The default index auto-merges an announcement PR when: only your own
namespace's `metadata.json` paths changed, you own that namespace (login
match or public membership in the org), `owner.id` matches your account
(live API check, spoof-proof against login recycling), every changed file
passes the schema, and the `ref` is reachable (the registry lists at
least one tag anonymously — publish before you announce). Anything else
falls to manual maintainer review. Full spec: [The Package Index][package-index].

## Editor schema support {#editor-schema}

`grim schema --kind config|publish|lock|mcp` prints a JSON Schema for
`grimoire.toml`, `publish.toml`, `grimoire.lock`, or the MCP server
descriptor (`mcp/<name>.toml`) — generated from grim's own parser, so it
accepts exactly what grim accepts. The same schemas are published to the docs site; adding a
`#:schema` directive on the first line of a TOML file gives a supporting editor
(Taplo, Even Better TOML) autocomplete and typo-flagging. Confirm the flags
with `grim schema --help`; see the [Editor schema support][editor-schema] docs
for the hosted URLs.

## Bundles

A bundle is a small `.toml` whose `[skills]` / `[rules]` / `[agents]`
tables list members by reference — the same shape as a `grimoire.toml`.
Build and release it like any artifact:

```sh
grim build ./python-stack.toml
grim release ./python-stack.toml ghcr.io/acme/python-stack:1.0.0 --pin
```

**Publish members before the bundle.** A bundle holds references to
already-published artifacts; consumers resolve those members at lock
time, so a member that is not on the registry yet breaks every consumer.

By default member tags stay floating and each consumer's `grim lock`
re-resolves them fresh. `--pin` instead freezes every floating member to
a digest at release time, making the bundle reproducible on its own —
the stronger guarantee for air-gapped or tunneled networks. Re-run the
pinned release to roll members forward.

## Catalog Metadata

Four optional fields make an artifact findable in `grim search` and the
TUI: `summary`, `keywords`, `description`, `repository`. A fifth field,
`deprecated`, retires a package *without* unpublishing it —
a non-empty notice keeps it resolving and installing while grim flags it
in `grim search`, the TUI, and on `grim add`; an empty or whitespace
value means not deprecated. A sixth, `replaced-by`, names the successor
artifact (authored independently of `deprecated`) — surfaced as
`replaced_by` in `grim search` / `grim describe --format json`; the value
must parse as a reference or the release fails with exit 65. You author
them all in the source file itself, so a release always publishes what the
file says. Two invariants hold for every kind:

- `keywords` is a single comma-separated **string** (`rust,lint`), never
  a YAML/TOML list — an OCI annotation value is a string.
- `repository` must be an `https://` URL; anything else fails the
  release with exit 65.

*Where* the fields live differs by kind (skill/agent: the frontmatter
`metadata` map; rule: top-level frontmatter; bundle: top-level TOML) —
see [the per-kind examples][metadata].

## Description Companion {#description-companion}

The in-tree README convention ([artifacts.md][artifacts-readme]) only
reaches tar-backed kinds — mcp and bundle publish a single JSON layer with
no file tree. For a README/logo/CHANGELOG that reads back uniformly
across **every** kind, `grim publish` can push a repository-level
**description companion**: a small doc bundle stored at an internal tag in
the same repository, read back via `grim fetch <ref> --description` (see
[consume.md](consume.md#inspecting)) — the tag itself is never something
you type.

Author it in `publish.toml` with a top-level `[description]` table (paths
relative to the manifest):

```toml
[description]
readme    = "README.md"
logo      = "assets/logo.png"
changelog = "CHANGELOG.md"
include   = ["docs/img/*.png"]   # extra README-referenced assets
```

- **Fan-out.** The top-level `[description]` publishes to **every**
  entry's repository. Override one entry with its own
  `[<kind>.<name>.description]` table (same schema), or opt it out with
  `description = false`. A manifest-wide `publish = false` inside the
  top-level table disables the companion for the whole run.
- **Convention fallback.** With no `[description]` table at all, grim
  probes the manifest directory for `README.md`, `CHANGELOG.md`, and
  `assets/logo.png|svg` / `logo.png|svg`; any hit publishes a companion
  automatically (opt out per entry with `description = false`). All
  members are optional — an explicit table that resolves to zero files is
  a data error (exit 65), but a conventional-probe miss is silent.
- Companion pushes ride the same `grim publish` batch, after the entry's
  own artifact push, and are idempotent — unchanged content republishes
  to the same digest. `--dry-run` previews the planned companion pushes
  alongside the artifact plan, and the `--format json` report carries a
  `descriptions` section (`{"items": [...], "descriptions": {...}, ...}`)
  beside the usual per-entry `items`.
- There is no separate publish command for it — re-running `grim publish`
  after a docs-only edit re-points the companion; the artifacts themselves
  skip-existing as usual.
- The companion's tag namespace is machine-owned: `grim release` /
  `grim publish` reject a user-supplied tag colliding with the reserved
  `__grimoire` namespace as a usage error (exit 64), before any network
  work. Companion paths that escape the manifest directory (`..`, absolute,
  or a symlink pointing outside) are a data error (exit 65).

Confirm the current schema with `grim schema --kind publish` and flags
with `grim publish --help`.

## Git Provenance

`build`, `release`, and `publish` take an opt-in `--git` flag that stamps the
publishing commit (revision, commit date, and the `origin` remote) onto the
manifest as standard OCI annotations, surfaced in the TUI detail pane and
`grim search --format json`. It is off by default so an ordinary re-release
stays idempotent; with `--git` a re-release from a different commit changes
the digest. A repo with no `origin` (or no HTTPS-resolvable remote) still
succeeds — revision and commit date are stamped and the source is just
omitted; only a non-git path or a missing `git` fails (exit 65). Confirm the
flag with `grim release --help` and see the [publishing guide][publishing]
for the trade-off.

## Authentication

grim reads and writes the same Docker-compatible credential store your
container tooling uses, so a prior `docker login` is already enough.
To log in with grim itself:

```sh
grim login ghcr.io -u alice                              # interactive prompt
echo "$GITHUB_TOKEN" | grim login ghcr.io -u alice --password-stdin
grim logout ghcr.io                                      # idempotent, exits 0
```

There is intentionally **no** `--password <value>` flag — a secret on
the command line leaks through the process list and shell history.

Credentials land in `$DOCKER_CONFIG/config.json` (default
`~/.docker/config.json`): a configured credential helper if present,
else grim *refuses* a plaintext write unless you opt in with
`--allow-insecure-store` (base64, not encryption; file mode `0600`).
By default the credential is verified against the registry before it is
stored — a wrong password fails at login time with exit 80 and nothing
persisted, an unreachable registry with exit 69. Verification proves the
registry accepts the credential, not that it grants push access to any
given repository. It can be skipped to store optimistically, and offline
mode skips it with a warning; confirm the flags with `grim login --help`.

The CI recipe — headless runner, no keychain, per-job isolation:

```sh
export DOCKER_CONFIG="$RUNNER_TEMP/docker"
echo "$REGISTRY_TOKEN" | grim login "$REGISTRY" -u "$REGISTRY_USER" \
  --password-stdin --allow-insecure-store
grim release ./code-review "$REGISTRY/acme/code-review:1.2.3"
grim logout "$REGISTRY"
```

With no positional registry, `login`/`logout` resolve `--registry`, then
`GRIM_DEFAULT_REGISTRY` — confirm with `grim login --help`.

## Further Reading

- [Publishing][publishing] — the full workflow: support directories,
  per-kind metadata, dry runs, bundle pinning.
- [The Package Index][package-index] — index spec, auto-merge rules,
  hosting your own.
- [Authentication][auth] — credential resolution, storage tiers, CI.
- [Command reference: build, release, login, logout][commands].

[publishing]: https://grimoire.rs/publishing.html
[package-index]: https://grimoire.rs/package-index.html
[metadata]: https://grimoire.rs/publishing.html#metadata
[batch-publish]: https://grimoire.rs/publishing.html#batch-publish
[editor-schema]: https://grimoire.rs/configuration.html#editor-schema
[auth]: https://grimoire.rs/authentication.html
[commands]: https://grimoire.rs/commands.html#build
[artifacts-readme]: https://grimoire.rs/artifacts.html#well-known-assets
