---
title: "Mirror upstream repositories"
description: "Republish skills, rules and agents from a git repository into your own registry with grim-mirror, and take them down again."
---
<!-- doc_type: how-to -->
<!-- doc_tier: integration -->

This page shows how to republish skills, rules, agents and bundles from a git
repository into your own registry with `grim-mirror`. It also shows how to keep
them current from CI.

> **Unstable.** `grim-mirror`, its `mirror.toml` file and its report may change
> in any minor release. What it writes onto each artifact is frozen: the
> [`mirror` table](./publishing.md#batch-publish-mirror) and the
> [three annotation keys](./artifacts.md#annotations-mirror). See
> [Stability](./stability.md#unstable).

A team that wants a vetted skill from a public repository has two bad
choices. It can copy the files into its own repository, which loses the
version, the commit and the licence. Or it can point every project at the
upstream repository. That gives it no pin it controls, and no way to pull a
withdrawn skill.

`grim-mirror` is the third choice. It reads the upstream repository at a
recorded commit, checks the licence, and publishes each unit through
[`grim publish`](./commands.md#publish) under your namespace. Every copy says
where it came from.

## What it does, and what it does not {#scope}

The mirror republishes **skills, rules, agents and bundles**. It is a
publisher, not a sandbox, so its limits are the point of the design:

- **No upstream code runs.** The mirror reads git objects with `git` and never
  checks out a working tree, so no filter driver, hook or attribute of the
  upstream runs. A symlink or submodule inside a unit refuses
  that unit. So does a Git LFS pointer, or a skill built from a template source
  (`*.src.md`, `*.tmpl`, `*.j2` and the like). The mirror only republishes
  what is installable as committed.
- **Executable kinds are skipped.** A Claude plugin's hooks, MCP servers,
  settings and commands, and its other components (bin, LSP servers, monitors,
  output styles), are reported as `skipped` and never republished. A plugin
  source that is not a path in the marketplace repository or a git
  subdirectory is skipped too.
- **A licence is required.** A unit with no recognisable licence, or one the
  allowlist does not admit, is `refused`. The mirror never publishes
  a unit it cannot name a licence for.
- **Bytes stay verbatim.** Upstream frontmatter is not edited to make
  provenance true. Provenance goes in the manifest annotations instead.
  A `dialect = "claude"` upstream is the one exception: its units pass
  through [`grim convert`](./commands.md#convert).

## Install it {#install}

`grim-mirror` has no releases yet. Install it from a pinned commit of the
grimoire repository, so a run is reproducible:

```sh
cargo install --locked --git https://github.com/grimoire-rs/grimoire --rev <sha> grim-mirror
```

Replace `<sha>` with the commit you reviewed. Put `git` and `grim` on `PATH`
as well: the mirror calls both. With `grim-mirror` on `PATH`, `grim mirror`
runs it through [plugin dispatch](./commands.md#plugins), and hands it the
path of that grim in `GRIM_BIN`. The plugin also runs standalone as
`grim-mirror`, which is how the examples below write it.

## Write a mirror file {#mirror-toml}

A `mirror.toml` names the registry to publish to and the upstream
repositories to read. This is the shipped example,
`crates/grim-mirror/examples/mirror.toml`, in pieces.

```toml
schema = 1

[mirror]
registry = "ghcr.io/acme/mirror"
licenses = ["MIT", "Apache-2.0"]
backfill = 3
```

`schema` must be `1`. `[mirror] registry` is `host[:port][/prefix]`, never a
URL: every repository lands under it. `licenses` is the allowlist of SPDX ids.
Leave it out to admit every OSI-approved id. `backfill` (1 to 100, default 1)
is how many past releases to publish the first time the mirror sees a unit.

Each `[[upstream]]` entry is one repository. This one lists its units by kind:

```toml
[[upstream]]
name = "anthropic-skills"
repo = "https://github.com/anthropics/skills"
skills = ["skills/*"]
exclude = ["skills/internal-*"]
license = "Apache-2.0"

[upstream.unit-licenses]
"skills/pdf" = "MIT"
```

`name` becomes part of every target repository, so it must match
`^[a-z0-9][a-z0-9-]{0,62}$`. `skills`, `agents` and `rules` are globs over
repository paths: a skill is a directory holding `SKILL.md`, an agent or rule
is a file. `exclude` removes matches. An entry needs at least one of the
three lists or a `marketplace`.

A Claude plugin marketplace is read instead of kind lists:

```toml
[[upstream]]
name = "community-plugins"
repo = "https://github.com/acme/claude-plugins"
dialect = "claude"
marketplace = ".claude-plugin/marketplace.json"
plugins = ["review-tools"]

[[upstream.bundle]]
name = "review-kit"
members = ["skills/review-*"]
```

`plugins` are globs over plugin names and are required with `marketplace`.
A plugin's repository URL taken from the marketplace must be `https://`. A
`file://` URL is admitted only when the marketplace's own `repo` is `file://`.
Any other URL is `skipped` as `source-unsupported`. A plugin pulled from another
repository records that repository, its commit and its commit date on its
artifacts. Its bundle names that repository too when the bundle's commit is a
member's from there. When the marketplace file changed last, the bundle names
the marketplace. An unreadable commit time is a `failed` row, never a 1970 date.
One broken plugin does not stop the rest. A repository that answers that it
does not exist, or lacks the pinned commit or ref, gives one `refused` row for
the plugin, kind `bundle`, reason `upstream-unavailable`. A fetch or read that
fails on the way gives one `failed` row of kind `bundle` instead. A timeout, a
network error, a refused login or unreadable objects all count, and the run
exits with that failure's code. An unreadable or oversize manifest, or a
submodule agent, gives `unsupported-entry`. The other plugins run and the
upstream stays `ok`.
`dialect = "claude"` runs each unit through `grim convert`. A `[[upstream.bundle]]`
publishes a bundle after its members, each pinned to the exact version just
mirrored. Members are globs over unit paths.

Last, an entry can be retired, which the [takedown](#takedown) section covers:

```toml
[[upstream]]
name = "old-rules"
repo = "https://github.com/acme/old-rules"
rules = ["rules/*.md"]

[upstream.retire]
reason = "the upstream repository was archived"
replaced-by = "ghcr.io/acme/mirror/community-plugins"
```

### Every key {#mirror-keys}

| Key | Meaning |
|-----|---------|
| `name`, `repo` | The entry's name, and its `https://` or `file://` URL with no userinfo and no `#` fragment. Names are unique in the file. |
| `ref` or `commit` | A branch or tag to read, or one 40-hex commit. They exclude each other. Default: the remote's default branch. |
| `dialect` | `agentskills` (default) or `claude`. |
| `version` | `auto` (default) or `snapshot`. See [versions](#versions). |
| `tag-pattern` | A regular expression whose first capture group is the version. Default `^v?(\d+\.\d+\.\d+)$`. |
| `backfill` | Per-entry override of `[mirror] backfill`. |
| `license`, `unit-licenses` | A declared licence for the entry, and per unit path. SPDX expressions. See [licences](#licences). |
| `skills`, `agents`, `rules`, `exclude` | Path globs. No absolute path, `..` or backslash. |
| `marketplace`, `plugins` | The marketplace file, and plugin-name globs. Each requires the other. |
| `bundle` | `[[upstream.bundle]]` tables of `name` and `members`. |
| `retire` | `reason`, and optionally `replaced-by` and `units`. |

An unknown key, anywhere, exits `78` with the full key path, for example
`mirror.toml: upstream[0].bogus: unknown key`. The message starts with the
path you gave `--file`; only the default `./mirror.toml` prints as `mirror.toml`.

### Target names {#target-names}

Repositories land in a fixed layout under your registry, never under a name
that imitates the upstream author:

| Unit | Repository |
|------|------------|
| Skill, rule or agent | `<registry>/<upstream>/<skills\|rules\|agents>/<name>` |
| Marketplace member | `<registry>/<upstream>/<plugin>/<skills\|rules\|agents>/<name>` |
| Bundle | `<registry>/<upstream>/bundles/<name>` |

Two units of one kind with one name in an entry are both `refused` as
`name-collision`.

### Licences {#licences}

The first level with evidence decides: a licence file or field in the unit, then
at its plugin (a plugin's own licence files count even without a `plugin.json`),
then at the repository root. For a plugin at the root of its repository, the
`license` field of its `plugin.json` joins the repository level, beside the root
licence files and `package.json`, so a root `LICENSE` that disagrees with it
refuses the unit. An id the mirror recognises beats any
declaration, with a `declaration-ignored` warning. A licence file it cannot
name refuses the unit unless `unit-licenses` declares one. With no evidence at
all, `unit-licenses` and then `license` supply the licence, and it is recorded
as `declared`. The mirror copies the licence file of the level that decided
into a unit that has none of its own, and only when that level's evidence is a
file, not a manifest field.

The decision is then held against `[mirror] licenses`. For an expression, `OR`
needs one admitted branch and `AND` needs every branch. A `LicenseRef-*` never passes.

### Versions {#versions}

In `auto` mode the mirror publishes upstream release tags. Tags that match
`tag-pattern` and read as strict `X.Y.Z` count. The highest `backfill` of
them above what is already published are published, oldest first. A tag that
moved to another commit since it was published is `refused` as `upstream-moved`,
because a version is immutable.

With no eligible tag, or with `version = "snapshot"`, the unit is published as
`<major>.<minor>.<stamp>`. `major.minor` come from the highest eligible tag
merged into the commit, or are `0.0`. The stamp is the commit time in UTC as
`YYYYMMDDHHMMSS`, which makes a patch of exactly 14 digits mark a snapshot. A
second run with nothing new publishes nothing.

## Check, then sync {#run}

Two commands, one file:

```sh
grim-mirror check
grim-mirror sync
```

`check` does everything but the push: it fetches, enumerates, decides the
licences, converts, and runs `grim publish --dry-run`. `sync` publishes. A
registry login with push rights is needed for `sync` only. See
[authentication](./authentication.md).

Put the flags before or after `check` or `sync`; both work.

| Flag | Effect |
|------|--------|
| `--file <path>` | The mirror file. Default `./mirror.toml`. |
| `--upstream <name>` | Restrict the run to that entry. Repeatable. An unknown name exits `64`. |
| `--work-dir <dir>` | Keep clones here, so the next run fetches instead of cloning. Default: a temporary directory. |
| `--fail-on <refused\|infra>` | Which outcomes make the exit non-zero. Default `refused` for `check`, `infra` for `sync`. |
| `--format <plain\|json>` | The report. Default `plain`. |

The default split is deliberate. A standing refusal is a defect in the list,
and `check` on a pull request should fail on it. A scheduled `sync` that went
red on one refused unit forever would hide the next outage, so it fails only
on infrastructure.

Mirroring needs the network. `GRIM_OFFLINE` set, or `grim --offline mirror`,
exits `81`.

## Read the report {#report}

`--format json` prints one document. `schema` is `1`, `mode` is `check` or
`sync`, `grim` and `grim_mirror` are the two versions, so a run is
reproducible from its report.

```json
{
  "schema": 1,
  "mode": "check",
  "grim": "0.16.1",
  "grim_mirror": "0.1.0",
  "upstreams": [
    {"name": "anthropic-skills", "repo": "https://github.com/anthropics/skills",
     "commit": "0123456789abcdef0123456789abcdef01234567",
     "status": "ok", "error": null, "warnings": []}
  ],
  "items": [
    {"upstream": "anthropic-skills", "plugin": null, "bundle": null,
     "kind": "skill", "name": "pdf", "path": "skills/pdf",
     "ref": "ghcr.io/acme/mirror/anthropic-skills/skills/pdf:1.2.0",
     "version": "1.2.0", "revision": "0123456789abcdef0123456789abcdef01234567",
     "status": "would-publish", "reason": null, "message": null,
     "license": {"expression": "MIT", "source": "detected"},
     "members": null, "warnings": []}
  ],
  "summary": {"published": 0, "would_publish": 1, "unchanged": 0,
              "refused": 0, "skipped": 0, "failed": 0}
}
```

Items sort by upstream, plugin, kind, name and version. The plain output is
a table of `Upstream`, `Kind`, `Name`, `Version`, `Status` and `Reason`.

An item's `status` is one of:

| Status | Meaning |
|--------|---------|
| `published` | Pushed this run (`sync`). |
| `would-publish` | Would be pushed (`check`). |
| `unchanged` | Already published at the same revision. |
| `refused` | The mirror declined it. `reason` says why and is a list defect. |
| `skipped` | A kind or component the mirror never republishes. It never changes the exit. |
| `failed` | Infrastructure: git, the registry, or grim failed. |

The `reason` vocabulary: `license-missing`, `license-unrecognised`,
`license-not-allowed`, `license-conflict`, `unsupported-build-step`,
`unsupported-entry`, `unsafe-path`, `invalid-name`, `name-collision`,
`upstream-moved`, `version-collision`, `bundle-drift`, `empty-bundle`,
`convert-failed`, `grim-refused`, `upstream-unavailable`, `kind-unsupported`,
`component-unsupported`, `source-unsupported`, `plugin-conflict`,
`commit-unreachable` and `ref-unknown`.

### Exit codes {#exit-codes}

| Code | Meaning |
|------|---------|
| `0` | Every upstream ran and nothing counts against `--fail-on`. |
| `64` | Usage error, or an unknown `--upstream` name. |
| `65` | `--fail-on refused` and at least one item is `refused`. |
| `69`, `74`, `75`, `80`, `81` and the like | The first infrastructure failure, in file order, with grim's own code when grim failed. A failure with no code exits `69`. |
| `78` | The mirror file is invalid. |
| `79` | The mirror file does not exist. |

The infrastructure code wins over `65`: a run that both refused a unit and
lost the registry exits with the registry's code. A drift check that cannot run
counts too. The already-published row then reads `failed` with a
`drift-check-failed` warning, and the run exits with that failure's code,
because the run could not confirm that version. When grim refuses the check,
the row reads `refused` as `grim-refused` instead. The codes are those of
[grim](./json-interface.md#error-document).

## Run it from CI {#ci}

This is an **example**, not a workflow grimoire ships. Adapt the version pins
and the registry to your list repository. `check` runs on pull requests and
fails on a refused unit. `sync` runs on a schedule on the default branch. Both
install the same pinned commit.

```yaml
name: Mirror
on:
  pull_request:
  schedule:
    - cron: "17 3 * * *"
  workflow_dispatch:

permissions:
  contents: read
  packages: write

env:
  GRIM_MIRROR_REV: "<sha>" # the reviewed commit of grimoire-rs/grimoire

jobs:
  check:
    if: github.event_name == 'pull_request'
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: grimoire-rs/setup-grimoire@v1
      - run: >
          cargo install --locked --git https://github.com/grimoire-rs/grimoire
          --rev "$GRIM_MIRROR_REV" grim-mirror
      - run: grim-mirror check --fail-on refused --format json

  sync:
    if: github.event_name != 'pull_request'
    runs-on: ubuntu-latest
    concurrency:
      group: mirror-ghcr.io-acme-mirror
      cancel-in-progress: false
    steps:
      - uses: actions/checkout@v4
      - uses: grimoire-rs/setup-grimoire@v1
      - run: >
          cargo install --locked --git https://github.com/grimoire-rs/grimoire
          --rev "$GRIM_MIRROR_REV" grim-mirror
      - name: grim login
        env:
          REGISTRY_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        run: |
          echo "$REGISTRY_TOKEN" | grim login ghcr.io -u "$GITHUB_ACTOR" \
            --password-stdin --allow-insecure-store
      - run: grim-mirror sync --format json
```

Two things matter more than the rest. The `concurrency` group names the
target registry prefix. The mirror assumes **one writer per prefix**, and a
second writer shows up as a revision mismatch, not as corrupt data.
And the job's own `GITHUB_TOKEN` is the registry credential, so a fork's pull
request, which gets a read-only token, can run `check` but never publish.
[Publishing from CI](./ci.md) covers the login step in full.

## Take something down {#takedown}

An upstream goes away, or its owner asks you to pull a unit. The answer
depends on what the request demands.

**Deprecate it, keep every pin working.** Add a `retire` table to the entry:

```toml
[upstream.retire]
reason = "the upstream repository was archived"
replaced-by = "ghcr.io/acme/mirror/other"
units = ["skills/old-tool"]
```

On the next `sync`, the mirror takes each unit of the entry. It re-stages the
highest published version from its recorded `revision` and `path`, and
publishes it one patch up (`1.2.0` becomes `1.2.1`) with
[`deprecated`](./publishing.md#metadata-deprecated) and
[`replaced-by`](./publishing.md#metadata-replaced-by) set. The new version is
staged with the current converter, so its payload can differ from the old one.
`units` lists target suffixes such as `skills/old-tool` for units that no
longer exist upstream and so cannot be enumerated. A suffix inside a marketplace
plugin is `<plugin>/skills/<name>`.

A consumer then sees the notice. The floating tags move onto the deprecated
version, so [`grim status --check`](./commands.md#status-check) reports the
artifact as deprecated, with its `replaced_by` reference, while every exact tag
and every `grimoire.lock` keeps resolving. A unit already deprecated is
`unchanged`, so the entry can stay in the file. Remove it once the run has
published.

**Delete it.** A takedown that demands removal is the registry's job: grim has
no delete command and the mirror adds none. Delete the repositories with your
registry's own tooling. When the upstream commit is already gone, the retire
run is `refused` as `upstream-unavailable`, and deletion is the only path left.

To leave the mirror for a native publish, retire the entry, then publish the
package yourself from a `publish.toml`. The `mirror` table records provenance
for a hand-written entry as well.

## Trust {#trust}

A mirrored artifact is **upstream-authored and operator-vouched**. You wrote
none of it, and the `com.grimoire.mirror.source` annotation says so. By
republishing under your namespace, you become its publisher of record: a
consumer who trusts your registry trusts what you mirrored.

grim's guarantee covers the bytes the mirror pushed, pinned by digest. It says
nothing about who wrote them. The mirror narrows what it takes in. It reads one
pinned commit per publish and records it as the `revision`.

It runs no upstream code, refuses a moved tag and republishes no executable
kind. It cannot judge whether the instructions in a skill are wise. Review what
you add to the list, and pin the commit you install the mirror from.
