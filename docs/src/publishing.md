# Publishing Skills and Rules

Consuming artifacts is only half of Grimoire. The other half is producing them:
turning a local skill directory or rule file into a versioned OCI artifact that
others can [`grim add`](./commands.md#add).

## Author locally

A **skill** is a directory containing a `SKILL.md` and any supporting files; a
**rule** is a Markdown file, optionally with a
[sibling support directory](#rule-support-dir); an
[**agent**](./agents.md) is a Markdown file defining a delegatable assistant;
an [**MCP server**](./mcp-servers.md) is a `mcp/<name>.toml` file describing
how to launch or reach a Model Context Protocol server; a
[**bundle**](./concepts.md#bundles) is a `.toml` file listing members.
Grimoire detects which one you mean from the path — a directory packs as a
skill, a `.md` file as a rule, a `.toml` file as a bundle — and `--kind`
overrides the guess when you need to. Two kinds **require** the flag because
their shape collides with another kind's: an agent needs `--kind agent`
(its `.md` shape is indistinguishable from a rule; see
[Agent Artifacts](./agents.md#publishing)), and an MCP server needs
`--kind mcp` (its `.toml` shape is indistinguishable from a bundle; see
[MCP Server Artifacts](./mcp-servers.md#publishing)). grim never guesses
either from content — it only nudges with a hint once the file's shape
(a `[server]` table, a `name`+`description` pair) makes the mismatch obvious.

### Rules with a support directory {#rule-support-dir}

An index rule often references extra context — examples, a schema, a script —
that does not belong inside the rule body. Put those in a folder beside the
rule that shares its stem, and Grimoire packs both into the one artifact:

```
rules/
  my-rule.md        # the index you pass to build/release
  my-rule/          # optional support directory, same stem
    examples.md
    schema.json
```

You still point [`grim build`](./commands.md#build) and
[`grim release`](./commands.md#release) at the index `.md` file — the sibling
directory is discovered automatically when it exists:

```sh
grim release ./my-rule.md ghcr.io/acme/my-rule:1.0.0
```

Every file under `my-rule/` rides along in the same layer and installs beside
the index (`.claude/rules/my-rule.md` + `.claude/rules/my-rule/…`), so the
index's relative links resolve on the consumer. Support files are copied
verbatim for every [client](./concepts.md#clients) — only the index is ever
transformed. A rule with no support directory packs to exactly the single
`my-rule.md` it always did.

### Agents with a README and logo {#agent-companions}

An [agent](./agents.md) is a single `.md`, but it may carry a `README.md` and a
`logo.png`/`logo.svg` from a sibling directory sharing its stem — the same
discovery as a rule's support directory, restricted to those well-known files:

```
agents/
  code-reviewer.md        # the index you pass to build/release
  code-reviewer/          # optional companion directory, same stem
    README.md             # human-facing readme (shown by catalog UIs)
    logo.png              # optional icon
```

Only `README.md`, `logo.png`, and `logo.svg` ride the layer (any other file in
the directory is ignored, so an agent never becomes an accidental multi-file
artifact). They pack under `code-reviewer/…`, so a consumer pulls them with the
same path shape as a skill or rule:

```sh
grim fetch ghcr.io/acme/code-reviewer:1.0.0 --path code-reviewer/README.md
```

The companions are **not** installed to a client — an agent installs as its
lone `.md` file; they exist for `grim fetch` and catalog UIs. An agent with no
companion directory packs to exactly the single `code-reviewer.md` it always
did.

MCP servers and bundles publish a single JSON layer with no file tree of their
own, so they carry no *in-tree* README. For a README that works uniformly across
every kind, publish a [repository description companion](#description-companion).

## Repository description companion {#description-companion}

The in-tree READMEs above ride each artifact's own layer, so they cover the
tree-backed kinds (skill, rule, agent) but not mcp or bundle. A **description
companion** is a repository-level channel that works for *every* kind: it is the
one home for all of a repo's descriptive data — a `README.md`, a `CHANGELOG.md`,
a logo, and any assets the README references — published to the reserved
`__grimoire` tag in the same repository as each artifact.

The companion is not a separate command — it rides
[`grim publish`](#batch-publish). After each entry's artifact is pushed, grim
(re)points that repository's `__grimoire` tag at a deterministic tar of the
repo's descriptive files. The companion is marked `com.grimoire.kind: desc`; it
is **not** an artifact kind, so it never installs, resolves, or appears in a
catalog, and its reserved `__grimoire` tag is hidden from every user-facing tag
listing (`grim describe` `tags[]`, the TUI version picker, catalog version
selection). Direct resolution of the tag still works. Because the pack is
byte-stable, republishing unchanged content produces an identical digest — the
registry stores nothing new (the tag is always re-pointed, never gated by
skip-existing).

### Conventional layout {#description-probe}

With no configuration at all, grim probes the manifest directory for the
conventional files and publishes a companion when it finds any:

| Source (relative to `publish.toml`) | Packed as |
|-------------------------------------|-----------|
| `README.md` | `README.md` |
| `CHANGELOG.md` | `CHANGELOG.md` |
| `assets/logo.png` / `assets/logo.svg` / `logo.png` / `logo.svg` (first hit) | `logo.png` / `logo.svg` |

Every member is optional — a repo with just a `README.md` publishes a
one-file companion. Probe misses are silent: a manifest directory with none of
these files simply publishes no companion, which is not an error.

### The `[description]` table {#description-table}

To decouple the companion from your repository layout — or to add extra
assets — declare a `[description]` table. Its paths are relative to
`publish.toml`, and the well-known members map their source onto the fixed
wire name, so your repo can lay files out however it likes:

```toml
[description]                     # optional; fans out to every entry
readme    = "docs/readme.md"      # → packed as README.md
logo      = "assets/brand.svg"    # → packed as logo.svg (by extension)
changelog = "CHANGELOG.md"        # → packed as CHANGELOG.md
include   = ["docs/img/*.png"]    # extra assets — keep their relative path
# publish = false                 # manifest-wide kill switch
```

`include` globs (`*`/`?` within a segment, `**` across segments) pull in
README-referenced assets; each hit keeps its manifest-relative path on the
wire. An explicit `readme`/`logo`/`changelog` path that does not exist is a
data error (exit 65) — an explicit config must not silently skip. A companion
path — a well-known member or an `include` hit — that resolves *outside* the
manifest directory (a `..` segment, an absolute path, or a symlink whose target
escapes the tree) is likewise a data error (exit 65), checked before any push;
a leading `./` is accepted (`./README.md` ≡ `README.md`).
A `[description]` table that resolves to zero files is a data error too;
`publish = false` disables the auto-companion for the whole manifest.

### Fan-out, override, and opt-out {#description-fanout}

A top-level `[description]` publishes the **same** companion to every entry's
repository. A per-entry table overrides it for one entry, and
`description = false` opts an entry out:

```toml
[description]
readme = "README.md"                 # every repo gets this by default

[skills.grim-usage]
repository = "grimoire-rs/skills/grim-usage"
[skills.grim-usage.description]      # per-entry override (same schema)
readme = "skills/grim-usage/README.md"

[mcp.grim]
repository = "grimoire-rs/mcp/grim"
description = false                  # this repo gets no companion
```

A `--dry-run` publish lists the planned companion pushes (`descriptions` in
the [JSON report](#batch-publish-report), digest `null`) without touching the
registry, so you can confirm the fan-out before it happens. Validation parity:
a dry run still containment- and size-checks every companion and packs it into
its layer, so a bad companion fails the dry run — only the registry push is
skipped, and either way zero registry mutations occur.

### Read it back {#description-read}

Read the companion with
[`grim fetch --description`](./commands.md#fetch-description) — the
reserved `__grimoire` tag is a grim-internal implementation detail; the
flag is the documented way to reach it, so nothing outside grim needs to
type or know the tag:

```sh
grim fetch ghcr.io/acme/mcp/postgres --description                              # every packed file inline
grim fetch ghcr.io/acme/mcp/postgres --description --format json | jq '.files[].path'
grim fetch ghcr.io/acme/mcp/postgres --description --out ./docs                 # unpack the tree to disk
```

`--format json` reports `kind: "desc"` and every packed file inline in
`files[]` — README, logo, changelog, and any README-referenced assets — in
one call, bounded by the same 8 MiB layer gate as any fetch. A repository
with no companion published returns a clean *not-found* error, so a
consumer can fall back to an in-tree README. [`grim describe`](./commands.md#describe)'s
`has_description` field answers "does this repository have one?" without
a probe fetch at all.

### Replication caveat {#description-replication}

The companion is a separate manifest under the repository's reserved
`__grimoire` tag, distinct from the artifact's version tags. That distinction
matters when you replicate a repository between registries.

A **single-tag** copy — [`skopeo copy`][skopeo] or [`oras cp`][oras] naming one
tag like `:1.2.3` — carries only that tag's manifest and blobs. It does **not**
follow the `__grimoire` tag, so the mirror ends up with the artifact but no
description companion; a later `grim fetch --description` against the mirror
returns *not-found*.

A **full-repository** sync — every tag, e.g. `skopeo sync` or `oras cp
--recursive` over the whole repository — carries the `__grimoire` tag along
with the version tags, so the companion survives. Mirror the whole repository
(or re-run `grim publish` against the destination) when you need the companion
to travel with the artifact.

The `__grimoire` namespace is a **grim-client-side convention**, not a
registry-enforced reservation. grim refuses to publish a `__grimoire` /
`__grimoire.<x>` reference itself, but any other OCI tool can still write that
tag directly — treat the namespace as reserved only within grim's own tooling,
not as a guarantee the registry upholds.

## Catalog metadata {#metadata}

[`grim search`](./commands.md#search) and the [TUI](./commands.md#tui) list
every match in a table. To make a result legible and findable, an artifact
carries seven pieces of catalog metadata, all optional:

| Field | Annotation | Purpose |
|-------|-----------|---------|
| `summary` | `com.grimoire.summary` | One-line blurb shown in the catalog (preferred over the description). |
| `keywords` | `com.grimoire.keywords` | Comma-separated terms that search matches. |
| `description` | `org.opencontainers.image.description` | The full description. |
| `license` | `org.opencontainers.image.licenses` | SPDX-style license identifier (e.g. `Apache-2.0`). |
| `repository` | `org.opencontainers.image.source` | HTTPS URL of the artifact's source repository ([details](#metadata-repository)). |
| `deprecated` | `com.grimoire.deprecated` | A deprecation notice; marks the package deprecated and flags it everywhere ([details](#metadata-deprecated)). |
| `replaced-by` | `com.grimoire.replaced-by` | A reference naming the successor artifact ([details](#metadata-replaced-by)). |

`grim search` shows the `summary` in place of the `description`, truncated to
fit the terminal; the full description stays in `--format json` and in piped
output. Search matches the repository, summary, description, **and** keywords,
so a query hits regardless of which one carries the term. Omit `summary` and
the catalog falls back to the description.

You author this metadata in the source file, so a `grim release` always
publishes whatever the file currently says — no separate flags to remember.
Where it lives differs by kind.

### In a skill {#metadata-skill}

A skill puts catalog metadata under the `metadata` map of its `SKILL.md`
frontmatter (the map the [Agent Skills](https://docs.claude.com/en/docs/agents-and-tools/agent-skills/overview)
format defines), separate from the top-level `description`:

```yaml
# code-review/SKILL.md
---
name: code-review
description: A thorough multi-pass reviewer that checks correctness, security, and style across the whole diff.
metadata:
  summary: Multi-pass code reviewer
  keywords: review,quality
  repository: https://github.com/acme/code-review
---
```

### In a rule {#metadata-rule}

A rule has no `description` field — that is derived from the body's first
heading or paragraph. `summary` and `keywords` sit at the top level of its
frontmatter:

```yaml
# rust-style.md
---
paths: ["**/*.rs"]
summary: Idiomatic Rust style rules
keywords: rust,lint
repository: https://github.com/acme/rust-style
---
# Rust Style
…
```

### In an agent {#metadata-agent}

An agent authors catalog metadata in its `metadata` map, like a skill; the
required `description` doubles as the full catalog description:

```yaml
# code-reviewer.md
---
name: code-reviewer
description: Reviews diffs for correctness, security, and style.
metadata:
  summary: Multi-pass diff reviewer
  keywords: review,quality
  repository: https://github.com/acme/code-reviewer
---
```

### In an MCP server descriptor {#metadata-mcp-server}

An [MCP server descriptor](./mcp-servers.md) authors every metadata field —
including `description` — as **top-level** TOML keys, the same shape as a
bundle rather than a skill or agent: there is no nested `metadata` map:

```toml
# mcp/acme-search.toml
description = "Acme's internal search index over MCP."
summary = "Acme search MCP server"
keywords = "acme,search,mcp"
repository = "https://github.com/acme/mcp-search"

[server]
transport = "http"
url = "https://mcp.acme.internal/search"
```

[`grim build`](./commands.md#build) and [`grim release`](./commands.md#release)
require `--kind mcp` for an MCP descriptor: its `.toml` shape is
bundle-shaped by default, and grim only nudges toward `--kind mcp` once it
notices a `[server]` table (`grim publish` needs no flag — a manifest
entry's kind is fixed by which table it sits in). See
[MCP Server Artifacts](./mcp-servers.md#publishing) for the full field
reference and validation rules.

### In a bundle {#metadata-bundle}

A [bundle](#bundles) sets the same keys at the top level of its `.toml`, above
the member tables. Here `description` overrides the otherwise-automatic
`grimoire bundle of N members`:

```toml
# python-stack.toml
summary = "Python dev stack"
keywords = "python,lint,test"
description = "Skills and rules for Python work"
repository = "https://github.com/acme/python-stack"

[skills]
code-review = "ghcr.io/acme/code-review:1"
[rules]
rust-style = "ghcr.io/acme/rust-style:2"
```

### Keywords are a string {#metadata-keywords}

`keywords` is always a single comma-separated string — in every kind — because
an OCI annotation value is itself a string. A YAML or TOML list is **not**
accepted; write `keywords: rust,lint`, not `keywords: [rust, lint]`.

### Repository URL {#metadata-repository}

`repository` links a published artifact back to the source repository it
came from. The value must be an `https://` URL (GitHub, GitLab, or any
forge) — a `git@…` or `http://` value fails the release with exit 65, the
same hard gate that guards [vendor metadata](./vendor-metadata.md#publish-validation).
The URL must **not** carry embedded credentials: an authored
`https://token@host/owner/repo` fails the release with exit 65 rather than
publishing the secret in the manifest. (grim never strips an *authored*
credential silently — only a git-derived `origin` remote is sanitized.)

On the wire it travels as the standard `org.opencontainers.image.source`
annotation, so registries that honor the key link the package to its
repository. When no `repository` is authored, grim keeps its previous
behavior and stamps the tagless release reference there instead. The
[TUI](./commands.md#tui) shows the URL in the detail pane and opens it
with the `o` key; `grim search --format json` exposes it as the
`repository` field.

### Deprecating a package {#metadata-deprecated}

`deprecated` retires a package without unpublishing it. Author a short
notice — ideally naming the replacement — and the package keeps resolving
and installing, but grim flags it at every point a consumer might reach for
it. The notice is the message; an empty or whitespace-only value means *not*
deprecated, so no annotation is emitted.

```yaml
# code-review/SKILL.md (skill / agent: under the metadata map)
metadata:
  deprecated: use acme/code-review-2 instead
```

```yaml
# rust-style.md (rule: top-level, like summary)
deprecated: superseded by rust-style-2
```

```toml
# python-stack.toml (bundle: top-level)
deprecated = "migrate to python-stack-2"
```

```toml
# mcp/acme-search.toml (MCP server: top-level, like bundle)
deprecated = "migrate to mcp/acme-search-v2"
```

Because the notice rides the `com.grimoire.deprecated` annotation on the
manifest, every surface reads it back without unpacking the artifact:

- [`grim search`](./commands.md#search) appends a comma-suffixed `deprecated`
  to the result's `Status` cell (e.g. `installed,deprecated`) and exposes the
  message as a `deprecated` field in `--format json`.
- The [TUI](./commands.md#tui) appends a yellow `⚠ deprecated` after the
  install-status label in the `Status` column (explained in the legend) and
  shows the full notice in the detail pane.
- [`grim add`](./commands.md#add) prints the notice on stderr when you
  acquire a deprecated reference (the add still succeeds).

A re-release with the notice removed clears the deprecation — the annotation
simply stops being emitted.

### Naming a replacement {#metadata-replaced-by}

`replaced-by` points a consumer at the successor artifact. It is authored
independently of `deprecated` — a package can name a replacement without
being deprecated (a rename that keeps working), or be deprecated with no
single successor — so the two keys are emitted and read separately. The
value must parse as an artifact reference; `grim build` / `grim release`
reject an unparseable value with exit 65, the same gate as the repository
URL. An empty or whitespace-only value emits no annotation.

```yaml
# code-review/SKILL.md (skill / agent: under the metadata map)
metadata:
  replaced-by: ghcr.io/acme/skills/code-review-2
```

```yaml
# rust-style.md (rule: top-level, like summary)
replaced-by: ghcr.io/acme/rules/rust-style-2
```

```toml
# python-stack.toml (bundle: top-level)
replaced-by = "ghcr.io/acme/bundles/python-stack-2"
```

The reference rides the `com.grimoire.replaced-by` annotation on the
manifest, so [`grim search`](./commands.md#search) and
[`grim describe`](./commands.md#describe) expose it as a `replaced_by` field
in `--format json` (`null` when none). It pairs naturally with `deprecated`:
deprecate the old package and name its replacement, and a consumer sees both
the notice and where to go next.

## Validate before you push

[`grim build`](./commands.md#build) validates and packs an artifact **without**
pushing it. Run it while iterating to catch a malformed skill before anyone
else sees it:

```sh
grim build ./code-review
grim build ./rust-style.md --kind rule
grim build ./code-reviewer.md --kind agent
grim build ./mcp/acme-search.toml --kind mcp
```

## Release

[`grim release`](./commands.md#release) validates, packs, and pushes to a
registry in one step. Give it the source path and the release reference:

```sh
grim release ./code-review ghcr.io/acme/code-review:1.2.3
```

### Cascade tags

A release does more than push one tag. From a `1.2.3` version it also moves the
**floating** tags that consumers track — `1`, `1.2`, and `latest` — to the new
digest. That is what lets a consumer who declared `:1` pick up `1.2.3` with a
plain [`grim update`](./commands.md#update).

The cascade fires automatically for a full semver and is skipped for a
non-version tag (`canary`, `edge`, a partial `1.2`). Two flags make it
explicit: `--cascade` asserts the cascade and rejects a non-semver tag with
exit 65 (a typo guard for CI), and `--no-cascade` publishes only the exact
tag even for a full semver — useful for a one-off version that should not
move `latest`. A prerelease (`1.2.3-rc.1`) is always exact-only: a release
candidate never becomes a floating version.

### Dry runs and overwrites

Preview the exact push plan — every tag and the digest each will point at —
without touching the registry:

```sh
grim release ./code-review ghcr.io/acme/code-review:1.2.3 --dry-run
```

An exact-version tag is immutable by default: if `1.2.3` already exists and
points at different bytes, the release refuses rather than rewrite history.
Pass `--force` only when you deliberately mean to move it.

## Git provenance {#git-provenance}

A published artifact rarely records which commit it was built from. Without
that link, tracing a registry tag back to the source — for an audit, a
rebuild, or a "why did this change" investigation — means guessing from
timestamps.

The opt-in `--git` flag closes that gap. Pass it to `grim build`,
`grim release`, or `grim publish` and grim reads the artifact's git working
tree and stamps three standard OCI annotations onto the manifest:

| Annotation | Value |
|---|---|
| `org.opencontainers.image.revision` | the `HEAD` commit SHA, suffixed `-dirty` when tracked files differ from `HEAD` |
| `org.opencontainers.image.created` | the commit date (RFC3339) — the *commit's* date, not a build clock |
| `org.opencontainers.image.source` | the `origin` remote, normalized to an `https://` URL — **conditional**: the git-derived URL is **not used** when you authored a [`repository`](#metadata-repository) value (the authored URL wins) or the repo has no HTTPS-resolvable remote. The annotation itself is still emitted from the usual fallback (authored `repository`, else the tagless release reference) |

```sh
grim release ./code-review ghcr.io/acme/code-review:1.2.3 --git
```

The git remote only fills `source` when you did **not** author a
[`repository`](#metadata-repository) value — an authored URL always wins, so
the two never collide. Any credentials embedded in the remote
(`https://token@host/...`) are stripped before the URL is written, so a token
in your `origin` URL never reaches the annotation. A path that is not inside a
git repository (or a host with no `git`) fails the release with exit 65 rather
than silently dropping the provenance you asked for.

There is a third outcome between those two. A repository with **no `origin`
remote** — or one whose remote does not resolve to an HTTPS URL (an SSH-only
host grim cannot rewrite, a `file://` remote, a bare local path) — is **not**
an error: `revision` and `created` are still stamped, and `source` is simply
omitted (falling back to whatever the authored `repository` or the tagless
release reference supplies). Only an absent repository or a missing `git`
fails.

### Why it is opt-in {#git-idempotent}

By default a re-release of identical content produces the same manifest
digest, so re-running a release is a harmless no-op (the
[overwrite guard](#dry-runs-and-overwrites) recognizes it). Embedding the
commit ties the digest to that commit: a re-release from a *different* commit
now changes the digest and is refused unless you pass `--force`. That is the
correct behavior — the provenance genuinely changed — but it is why git
provenance is something you ask for, not a silent default. The commit *date*
(not a wall-clock build time) keeps a re-release from the **same** commit
fully idempotent.

Every read surface shows the provenance back: the [TUI](./commands.md#tui)
detail pane adds `Revision:` and `Created:` rows, and
[`grim search --format json`](./commands.md#search) exposes `revision` and
`created` fields.

## Publishing bundles {#bundles}

A [bundle](./concepts.md#bundles) groups skills, rules, and
[agents](./agents.md) so consumers declare one reference instead of a dozen.
You author it as a small TOML file whose `[skills]`/`[rules]`/`[agents]`
tables list the members — the same shape as a `grimoire.toml`:

```toml
# python-stack.toml
[skills]
code-review = "ghcr.io/acme/code-review:1"

[rules]
rust-style = "ghcr.io/acme/rust-style:2"

[agents]
code-reviewer = "ghcr.io/acme/code-reviewer:1"
```

Members published beside the bundle can use
[deployment-relative references](./artifacts.md#bundle-relative-refs)
(`./name:tag`, `../skills/name:tag`) instead of fully-qualified ones —
they resolve at install time against wherever the bundle was pulled from,
so the bundle survives mirroring and enforced
[`--registry host/prefix`](#batch-publish-namespace) namespaces. A
relative member that would escape the registry root fails the release
(exit 65).

[`grim build`](./commands.md#build) validates it (a `.toml` path packs as a
bundle), and [`grim release`](./commands.md#release) pushes it with the same
cascade tags as any other artifact:

```sh
grim build ./python-stack.toml
grim release ./python-stack.toml ghcr.io/acme/python-stack:1.0.0
```

### Floating or pinned members {#pin}

By default the bundle stores its members exactly as written — floating tags stay
floating, and each consumer's [`grim lock`](./commands.md#lock) re-resolves them
fresh. Add `--pin` to resolve every floating member to a digest at release time
and freeze it into the published bundle:

```sh
grim release ./python-stack.toml ghcr.io/acme/python-stack:1.0.0 --pin
```

A pinned bundle is reproducible on its own: it always expands to the exact same
member digests, even on an air-gapped or tunneled network that cannot re-resolve
a tag. Re-run the release (a cron job tracking `:stable`, say) to roll the
pinned members forward. A
[deployment-relative member](./artifacts.md#bundle-relative-refs) is
resolved against the release target and then pinned absolute —
reproducibility forfeits its late binding.

## Batch publishing with a manifest {#batch-publish}

When a repository contains more than one package, releasing them one by one
with `grim release` means maintaining a shell script (or CI job) that
re-invents version tracking, ordering, and idempotent re-runs. That is a
generic capability dressed as project-specific tooling.

`grim publish` is the built-in alternative: it reads a `publish.toml`
manifest, validates the whole set before touching the registry, then
releases each entry in a fixed order.

### The publish.toml format {#batch-publish-manifest}

A manifest has one required top-level field — `registry` — and up to five
kind tables. Each table entry is a sub-table keyed by name with a
`version` field:

```toml
#:schema https://grimoire.rs/schemas/grim-publish.schema.json
registry = "ghcr.io"              # required; overridden by --registry

[skills.grim-usage]
version = "0.1.1"                  # strict X.Y.Z

[rules.custom-rule]
version = "0.2.0"
path = "shared/custom-rule.md"     # optional — overrides the conventional path

[agents.helper]
version = "0.1.0"

[mcp.acme-search]
version = "1.0.0"

[bundles.grim-essentials]
version = "0.1.0"
pin = true                         # optional, bundle entries only; default false
```

#### One version for the whole catalog {#batch-publish-version}

A catalog whose packages release together shouldn't repeat the same
version five times — that's five places to forget on the next bump. An
optional top-level `version` covers every entry that omits its own (or
sets the literal `${version}`, which resolves to the same value); an
explicit per-entry `version` always wins:

```toml
registry = "ghcr.io"
version = "0.9.0"                  # catalog-wide

[skills.grim-usage]                # no version → 0.9.0

[rules.custom-rule]
version = "${version}"            # explicit reference → 0.9.0

[mcp.acme-search]
version = "1.0.0"                  # per-entry override wins
```

For CI runs that publish from a git tag, `grim publish --version <ref>`
overrides the manifest's top-level `version` for that run. Every version
input — the flag, the top-level value, and per-entry values — first has
the manifest's `version_prefix` (default `"v"`) stripped when present, so
`--version v1.2.3` (a typical tag ref) publishes tag `1.2.3`. A semver
`--version` publishes the plain `X.Y.Z` form; a **non-semver** `--version`
(e.g. `canary`) is instead a movable channel tag applied to every entry — see
the [Flags](#batch-publish-flags) table. A different tagging convention
sets its own prefix:

```toml
version_prefix = "release-"        # release-1.2.3 → 1.2.3
```

An entry that ends up with no version anywhere — no per-entry value, no
top-level `version`, no `--version` — is a data error (exit 65) naming
the entry.

The `registry` value is a plain host (e.g. `ghcr.io`, `localhost:5000`), not a
full reference. All entries in the manifest publish to the same registry.
Only the `--registry` *flag* may carry a repository prefix after the host —
see [Repository namespace](#batch-publish-namespace).

Entry names must start with a character in `[a-z0-9]` and contain only
`[a-z0-9._-]` in the remainder. Uppercase letters, slashes, and `..`
components are all rejected at validation time (exit 65) — they would
produce an invalid OCI repository segment or a path traversal hazard. Unknown
fields in the manifest or in any entry sub-table are a hard parse error
(`deny_unknown_fields`): a typo like `versions` instead of `version` exits
immediately rather than silently using a default.

The first line above is a [Taplo](https://taplo.tamasfe.dev/) /
[Even Better TOML](https://marketplace.visualstudio.com/items?itemName=tamasfe.even-better-toml)
`#:schema` directive that binds the manifest to its published [JSON
Schema](https://grimoire.rs/schemas/grim-publish.schema.json),
so a supporting editor autocompletes keys and flags a typo before you ever run
`grim publish`. The schema is generated from grim's own manifest parser — see
[Editor schema support](./configuration.md#editor-schema) for both schema URLs
and [`grim schema`](./commands.md#schema) to print one locally.

### Repository namespace {#batch-publish-namespace}

By default, each entry pushes to `{kind-subdir}/{name}` under the
manifest's registry — a skill named `hearth` publishes to
`registry/skills/hearth`. Most self-hosted or single-user registries
work fine with this convention. Multi-tenant SaaS registries — such as
the [GitLab Container Registry][gitlab-registry] — require every image
to live under a group-and-project path, making the default layout
inaccessible.

Two optional fields let you replace the `{kind-subdir}` segment with an
arbitrary namespace path, so a publish manifest can target any registry
layout.

**Manifest-level `repository_prefix`** — a string applied to every entry
that does not set its own `repository`. The published repository becomes
`{repository_prefix}/{name}`; the prefix replaces the conventional
`{kind.subdir()}` segment. Registry-relative, no tag.

**Per-entry `repository`** — a string inside a `[skills.<name>]`,
`[rules.<name>]`, `[agents.<name>]`, or `[bundles.<name>]` sub-table.
The value is used verbatim as the full repository path; the entry name is
**not** appended — the same way `grim release` uses the repository portion
of its positional `registry/repo:version` reference verbatim. Wins over
`repository_prefix` when both are set.

Resolution precedence per entry (highest first):

1. per-entry `repository` (full path, name not appended)
2. manifest `repository_prefix` → `{prefix}/{name}`
3. default → `{kind.subdir()}/{name}` (unchanged backward-compatible behavior)

**CLI-enforced prefix** — a third, outer layer set per run rather than in
the manifest. When the global `--registry` flag value carries a path after
the host (`--registry registry.gitlab.com/durzn/hearth`), the first `/`
splits it: the host overrides the manifest `registry`, and the rest becomes
an enforced namespace prepended to *every* entry's resolved repository —
whichever branch above produced it, including a verbatim per-entry
`repository`:

```console
$ grim publish --registry registry.gitlab.com/staging
# manifest repository_prefix = "hearth/skill", skill hearth
#   → registry.gitlab.com/staging/hearth/skill/hearth
# per-entry repository = "custom/path"
#   → registry.gitlab.com/staging/custom/path
# neither field, skill bar
#   → registry.gitlab.com/staging/skills/bar
```

This lets a CI pipeline force a whole publish run under a namespace (a
staging area, a GitLab group/project) without editing the manifest. The
manifest `registry` field itself stays a plain host — a path inside it is
still rejected (exit 65).

```toml
#:schema https://grimoire.rs/schemas/grim-publish.schema.json
registry = "registry.gitlab.com"
repository_prefix = "durzn-technology/hearth/skill"

[skills.hearth]
version = "0.2.0"
# publishes to: registry.gitlab.com/durzn-technology/hearth/skill/hearth

[skills.other-skill]
version = "0.1.0"
repository = "durzn-technology/hearth/skill/other-skill"
# per-entry form — identical effect for this entry, wins over repository_prefix
```

The reporter's working example: registry `registry.gitlab.com`, prefix
`durzn-technology/hearth/skill`, skill `hearth` → resolves to
`registry.gitlab.com/durzn-technology/hearth/skill/hearth`.

**Charset rules** — each `/`-separated segment of both fields (and of the
`--registry` path portion) must match
the OCI name grammar: runs of `[a-z0-9]` joined by a single `.` or `_`, a
double `__`, or a run of `-`, with no leading, trailing, or doubled
separator. A leading or trailing `/`, empty `//` segments, `.` or `..`
segments, an embedded `:`, uppercase, and a path longer than 255 characters
are all rejected at manifest validation time with exit 65 (data error). An
invalid prefix or repository aborts the whole manifest before any push.

A manifest with neither field is unchanged: `ghcr.io/skills/grim-usage`
style paths are the default and remain fully backward compatible.

### Conventional source layout {#batch-publish-layout}

When `path` is omitted, grim derives the source path from the entry name and
kind, relative to the manifest's directory:

| Kind | Conventional path |
|------|-------------------|
| skill | `skills/{name}/` |
| rule | `rules/{name}.md` |
| agent | `agents/{name}.md` |
| mcp | `mcp/{name}.toml` |
| bundle | `bundles/{name}.toml` |

The `path` field overrides this convention for entries whose source lives
elsewhere.

### Kind ordering {#batch-publish-ordering}

Entries publish in a fixed kind order — skills, then rules, then agents,
then [MCP servers](./mcp-servers.md), then bundles — alphabetical within
each kind. Bundle entries land last by design: a bundle holds references
to already-published members, and consumers resolve those members at lock
time. Publishing a bundle before its members would produce a bundle that
references artifacts that do not yet exist.

### Skip-existing default and --force {#batch-publish-skip-existing}

By default, `grim publish` skips any entry whose exact-version tag already
exists on the registry — the push is a success no-op and nothing moves. This
makes the command safe to re-run from the top: only entries whose version was
bumped in the manifest since the last run actually push anything.

`--force` replaces the default with the opposite behavior: it moves an
existing exact-version tag that points at a different digest. The two modes
are mutually exclusive — `--force` and skip-existing cannot be combined.

This rule is now uniform for **every** value, including a channel
`--version` (see below): a channel like `canary` skips-existing by default
and needs `--force` to move, exactly like a semver release. There is no
special-cased always-moving tag.

### Flags {#batch-publish-flags}

| Flag | Description |
|------|-------------|
| `--manifest <path>` | Manifest file to read (default: `./publish.toml`). |
| `--dry-run` | Validate and plan without pushing. Prints what would be pushed. Companions are still containment- and size-checked and packed, so a bad companion fails the dry run; zero registry mutations occur either way. |
| `--force` | Move existing exact-version tags instead of skipping them. |
| `--only <name>` | Publish only the named entry (repeatable). A name absent from the manifest exits 65. |
| `--version <version>` | The single version source for the run. A **semver** value overrides the manifest's top-level `version` (entries with their own `version` keep it) and every entry cascades; a **non-semver** value (e.g. `canary`) is a movable channel tag applied to every entry uniformly, with no cascade. A prerelease/build-metadata value, a reserved cascade-float shape (`latest`, a bare major, or `major.minor`), or a value that is not a legal OCI tag exits 65 rather than being treated as a channel — see [Validation and fail-fast](#batch-publish-validation). The manifest's `version_prefix` (default `v`) is stripped first, so `--version v1.2.3` publishes `1.2.3`. See [One version for the whole catalog](#batch-publish-version). |
| `--cascade` / `--no-cascade` | Control the rolling cascade (`X.Y.Z` → `X.Y`, `X`, `latest`) for the whole run. Neither flag is the default: cascade automatically for a semver `--version`, single tag for a channel. `--cascade` asserts a semver release and exits 65 if combined with a channel value; `--no-cascade` publishes only each exact version tag. |
| `--registry <ref>` | The [global `--registry` flag][global-options] overrides the manifest's `registry` value for this run. The value may carry a repository prefix after the host (`host/group/project`): the host overrides the manifest registry and the rest is an enforced namespace prepended to every entry's repository — see [Repository namespace](#batch-publish-namespace). `GRIM_DEFAULT_REGISTRY` and the config-file `default_registry` do **not** override the manifest — `registry` is explicit input, like a fully-qualified reference. Only the flag tier wins. |
| `--announce` | After a fully successful, non-dry-run publish, announce the published packages to a [package index](./package-index.md): metadata pointers on a topic branch, pushed, with the PR/MR opened via the forge REST API (GitHub/GitLab, enterprise instances included), via git push options on a token-less GitLab host, or left as a branch on a plain git host. Configured by the optional `[announce]` manifest table (`repository`, `forge`, `host`, `api_url`, `namespace`, `owner_id`) plus CI auto-detection — [resolution chains](./package-index.md#announcing). An unreachable index or failed API call after a successful publish exits 69 (the packages **are** published; retry the announce); announce misconfiguration exits 64. The completed outcome — including the deterministic topic branch — is machine-readable in the JSON report ([Report output](#batch-publish-report)). |
| `--announce-repo <url>` | Override the index repository `--announce` targets (default: the manifest's `[announce] repository`, else `https://github.com/grimoire-rs/index`). Requires `--announce`. |

### Validation and fail-fast {#batch-publish-validation}

`grim publish` validates the whole manifest before any push: every resolved
`version` — after [inheritance and prefix stripping](#batch-publish-version) —
must be strict `X.Y.Z` semver, every source path must exist, and `pin = true`
is rejected on non-bundle entries (exit 65 for each). Only after the full
manifest passes does the first network call happen.

One check runs **before** every shape check below and exits **64** (usage), not
65: a `--version` channel value in grim's reserved namespace — `__grimoire` or
`__grimoire.<x>` — is refused up front, so a channel release can never overwrite
a repository's [description companion](#description-companion) tag. This is the
lone usage error among the manifest checks; every sibling condition below is a
data error (65).

Several additional conditions exit 65 at validation time:

- **Empty manifest** — a manifest that declares no entries in any kind table
  exits 65 with "no packages declared in manifest". Grim treats this as a
  likely wrong-file mistake rather than a valid no-op.
- **Oversized manifest** — a manifest file larger than 64 KiB is rejected
  before parsing. This is an unconditional limit, not a warning.
- **Prerelease or build-metadata `--version`** — a value like `1.2.3-rc.1`
  or `1.2.3+build` parses as semver but is not strict `X.Y.Z`. The
  manifest forbids prerelease/build entry versions, so grim rejects the
  value outright instead of silently treating it as a channel tag.
- **Reserved cascade-float shape** — a `--version` channel value equal to
  `latest`, a bare major (`1`), or a `major.minor` (`1.2`) is rejected.
  Those tags are managed automatically by a real semver release
  (`X.Y.Z` → `X.Y`, `X`, `latest`); a channel aliasing one would collide
  with the machine-owned float namespace.
- **Illegal OCI tag charset** — a `--version` channel value that does not
  match `[A-Za-z0-9_][A-Za-z0-9._-]{0,127}` — for example a slash-bearing
  CI ref like `feature/foo` — is rejected before it ever reaches the
  registry.

During the release run the command is fail-fast: the first failing entry
stops the batch. The report still renders — completed entries show their
status (`pushed`, `skipped`, or `dry-run`), the failed entry shows `failed`,
and remaining entries are unreported. Because skip-existing is the default,
re-running from the top after a fix pushes only what is left.

### Report output {#batch-publish-report}

The plain report is one table (Kind | Ref | Digest | Tags | Status); the
announce outcome stays human prose on stderr. `--format json` emits a
wrapper object on stdout:

```json
{
  "items": [
    {
      "ref": "ghcr.io/acme/skills/code-review:1.2.0",
      "kind": "skill",
      "digest": "sha256:…",
      "tags": ["1.2.0", "1.2", "1", "latest"],
      "status": "pushed"
    }
  ],
  "descriptions": {
    "items": [
      {
        "ref": "ghcr.io/acme/skills/code-review:__grimoire",
        "repository": "ghcr.io/acme/skills/code-review",
        "digest": "sha256:…",
        "files": ["README.md"]
      }
    ]
  },
  "announce": {
    "outcome": "pull-request",
    "branch": "announce/acme-1a2b3c4d",
    "url": "https://github.com/grimoire-rs/index/pull/7"
  }
}
```

`items` carries one object per manifest entry processed, in publish
order. `descriptions` carries the [description companion](#description-companion)
pushes this run, one per distinct target repository, in the same
`{"items": [...]}` envelope as every multi-item report; `items` is empty
when no companion was resolved for this run. Each entry's `digest` is
`null` under `--dry-run` (the preview lists the planned push without
touching the registry). `announce` carries the completed `--announce`
outcome: `outcome`
is `pull-request`, `branch-pushed`, or `up-to-date`; `branch` — the
deterministic topic branch on the index repository — is always present;
`url` is always present and non-null only for `pull-request`. `announce` is `null` whenever the
announce step did not complete: `--announce` not passed, a dry run, a
fail-fast stop, or an announce failure (which still exits 69 with the
entries rendered). A CI pipeline that needs the pushed branch — for
example to trigger a downstream index-validation pipeline — reads it
from here instead of parsing stderr.

### Example run {#batch-publish-example}

```sh
# Preview the full publish plan — zero writes
grim publish --dry-run

# Release everything in publish.toml, skip already-published versions
grim publish

# Release only one package
grim publish --only grim-usage

# Push every entry under a movable canary channel tag (no cascade);
# re-running is a no-op unless you add --force
grim publish --version canary
```

### Manifest vs bundle disambiguation {#batch-publish-disambiguation}

A `publish.toml` and a bundle `.toml` are structurally different: a manifest
has a top-level `registry` string and per-entry sub-tables with `version`; a
bundle has flat `name = "reference"` strings in its kind tables. The schemas
are disjoint and each parser rejects the other's input.

If you point `grim publish` at a bundle file, the command detects the shape
and reports: "this looks like a bundle source file; use `grim release --kind
bundle`". If you point `grim release` at a publish manifest, the bundle
reader returns the mirror hint. Neither silently misparses the other's format.

## Authenticate {#authenticate}

Grimoire pushes over standard OCI, so it reuses your existing registry
credentials — the same login your container tooling uses. Authenticate once
with your registry (for example, `docker login` against [GitHub Container
Registry][ghcr]) and `grim release` inherits it.

<!-- external -->
[ghcr]: https://docs.github.com/en/packages/working-with-a-github-packages-registry/working-with-the-container-registry
[gitlab-registry]: https://docs.gitlab.com/ee/user/packages/container_registry/
[skopeo]: https://github.com/containers/skopeo
[oras]: https://oras.land/docs/commands/oras_cp

<!-- internal -->
[global-options]: ./commands.md#global-options
