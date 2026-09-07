---
title: Add a company registry beside the public one
description: Add a company OCI registry next to the public index, narrow what each source shows, and reference an artifact across both.
---
<!-- doc_type: how-to -->
<!-- doc_tier: integration -->

This page declares your company's OCI registry alongside the public package
index. It narrows what each source lists while you browse, then references one
artifact across both. By the end you can browse only your own namespace and
still install anything either source holds.

## Before you start

You need `grim` installed and a project that already holds a `grimoire.toml`.
Run `grim init` if it does not. You also need the locator of your company
source, which is an OCI registry host such as `registry.acme.example`.

A source that requires a credential needs one before grim can read it.
`grim login` stores a credential per host, covered in
[Authentication](../authentication.md).

## Declare the company source

Add the company registry, filtered to the namespaces you browse, and mark it
the default.

```sh
grim config registry add company --oci registry.acme.example --include 'engineering/**' --include 'marketing/**' --default
```

```text
Action          Key               Value                  Scope    Dry Run
registry-added  registry.company  registry.acme.example  project  false
```

An entry declares exactly one locator. Use `--oci` for a registry host and
`--index` for a package index. The `default` key marks the entry short
references expand against, and at most one entry may set it. The full field
table is at [Multiple registries](../configuration.md#multiple-registries).

## Narrow what each source shows

`include` and `exclude` are glob lists on one entry. A repository is shown when
`include` is empty or one `include` pattern matches it, and no `exclude`
pattern matches. Where both match, `exclude` wins.

The entry above browses the two namespaces it was given, so a search lists
both:

```text
$ grim search --refresh
Kind   Repo                                                  Summary       Version  Status
skill  registry.acme.example/engineering/skills/diff-review  Diff-review.  1.0.0    not-installed
skill  registry.acme.example/marketing/skills/brand-voice    Brand-voice.  1.0.0    not-installed
```

Replace that list with the one namespace you want:

```text
$ grim config registry set company --include 'engineering/**'
Action        Key               Value      Scope    Dry Run
registry-set  registry.company  include=1  project  false
```

A repeated list flag replaces the whole list rather than appending to it, so
`marketing/**` is gone. The same search over the same source lists one row:

```text
$ grim search --refresh
Kind   Repo                                                  Summary       Version  Status
skill  registry.acme.example/engineering/skills/diff-review  Diff-review.  1.0.0    not-installed
```

A browse filter is not access control. It narrows browsing and nothing else, so
a full reference to a repository the filter hides still resolves, locks and
installs. The `marketing` namespace the search no longer lists adds cleanly:

```sh
grim add registry.acme.example/marketing/skills/brand-voice:1
```

That run prints a `Kind | Name | Pinned | Status` row reading `added`, with the
digest it resolved in the `Pinned` column. Watch it happen in the recording at
the foot of this page.

The glob dialect, and the two candidate strings every pattern is tested
against, are at [Browse filters](../configuration.md#browse-filters).

## Add the public index beside it

The public package index is a second source, declared with `--index` because
its locator is a URL rather than a registry host.

```sh
grim config registry add public --index https://index.grimoire.rs
```

```text
Action          Key              Value                      Scope    Dry Run
registry-added  registry.public  https://index.grimoire.rs  project  false
```

Both runs write into one `[[registries]]` array in `grimoire.toml`. The file
below is the state after the narrowing step above, so `company` carries the one
include that step left it with:

```toml
#:schema https://grimoire.rs/schemas/grimoire-config.schema.json
[options]
clients = ["claude"]

[[registries]]
alias = "company"
oci = "registry.acme.example"
include = ["engineering/**"]
default = true

[[registries]]
alias = "public"
index = "https://index.grimoire.rs"

[skills]

[rules]
```

Read the declared set back:

```sh
grim config registry list
```

```text
Alias    Type      Source                     Default  Filters               Insecure
company  registry  registry.acme.example      true     1 include, 0 exclude  false
public   index     https://index.grimoire.rs  false    —                     false
```

`Type` reads `registry` for an entry declared with `--oci`, and `index` for one
declared with `--index`. `Filters` counts the globs on that entry, and a dash
means the entry shows everything its source lists.

## Reference an artifact across sources

### Short references

A short reference is a bare name with no registry host in it. It expands to the
default registry followed by exactly what you typed, verbatim. Nothing is
inserted and nothing is dropped:

```text
$ grim add diff-review
could not infer the kind of 'registry.acme.example/diff-review:latest'; pass --kind skill|rule|agent|bundle|mcp
$ echo $?
65
```

The name expanded to `registry.acme.example/diff-review`, which is not where
the artifact lives. Its real path is `engineering/skills/diff-review`, two
levels deeper than a short reference can reach.

Exit `65` is the data code, and this is the registry-reference case. The same
failure against a local path exits `64` instead, which
[one directory every agent reads](./shared-skills.md) covers.

grim takes the default from the first rung that is set. The `--registry` flag
comes first, and it takes a locator rather than an alias. Then the primary
`[[registries]]` entry, which is the one marked `default = true`, or the first
registry-kind entry when none is marked.

The rest of the ladder applies only when no `[[registries]]` entry is a
registry. Then it is `GRIM_DEFAULT_REGISTRY`, then the project
`default_registry`, then the global one, then the built-in
`ghcr.io/grimoire-rs`. So declaring one `[[registries]]` entry takes the
environment variable out of the decision.

An index source never supplies that default, because an index locator is a URL
rather than a registry host.

### Qualified references

A qualified reference is `alias/repo`, and grim substitutes the alias with that
entry's locator. Against an `oci` entry the result is a registry host followed
by a repository path, which parses.

Against an `index` entry, `alias/repo` expands to the index URL, which is not a
repository name. The command exits `65` while parsing, before it contacts
anything:

```text
$ grim add public/skills/diff-review:1
invalid identifier 'https://index.grimoire.rs/skills/diff-review:1': repository must match the OCI name grammar: lowercase [a-z0-9] runs joined by '.', '_', '__', or '-', with no leading, trailing, or doubled separator
$ echo $?
65
```

The refusal is intended. An index lists artifacts rather than hosting them, and
its rows carry their own fully-qualified references. One index routinely spans
several registry hosts, so an index alias has no single host to expand to.

Use the row's fully-qualified reference instead.
[`grim search`](../commands.md#search) prints it in the `Repo` column, and it
works against every source. The reference statement is at
[Qualified references](../configuration.md#qualified-references).

## See it run

<div data-cast="/casts/registries.cast" data-cast-poster="npt:0:03"></div>
<noscript><a href="/casts/registries.cast">Download the recording</a></noscript>

## Next steps

- [Pin a team and gate CI](./team-ci.md): committing a lock so every developer
  and the CI runner install the same digests.
- [Package index](../package-index.md): what an index publishes, and how a row
  carries its own reference.
- [Host your own index](../hosting-an-index.md): standing up the company source
  this page declares.
