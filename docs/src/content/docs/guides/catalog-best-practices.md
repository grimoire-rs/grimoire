---
title: "Catalog layout and naming"
description: "Arrange a repository of skills, rules and agents so one publish run pushes every artifact and your index picks each one up under the right name."
---
<!-- doc_type: how-to -->
<!-- doc_tier: integration -->

Arrange a repository of skills, rules and agents so that one `grim publish`
run pushes every artifact. This page covers the paths grim derives, the
namespace you pick, and the name your index keys on.

## One directory per artifact

When an entry omits `path`, grim derives the source path from the entry
name and its kind, relative to the manifest's own directory.

| Kind | Conventional path |
|---|---|
| skill | `skills/{name}/` |
| rule | `rules/{name}.md` |
| agent | `agents/{name}.md` |
| mcp | `mcp/{name}.toml` |
| bundle | `bundles/{name}.toml` |

So `[skills.code-review]` reads from `skills/code-review/`. The common
mistake is nesting one level too deep and producing `skills/skills/`, which
grim cannot find. Set `path` on the entry when a source genuinely lives
somewhere else.

## Where the manifest sits

Put `publish.toml` at the repository root. Every conventional path above is
relative to the manifest file, so moving the manifest moves the whole tree
grim looks in.

## Choosing the registry namespace

Three fields decide the published repository path.

- `registry` is a bare host, such as `ghcr.io`. It carries no path.
- `repository_prefix` applies to every entry that sets no `repository` of
  its own. It replaces the conventional kind segment.
- A per-entry `repository` is used verbatim. The entry name is not
  appended, and it wins over `repository_prefix`.

The kind segment buys one thing: room for the same name to exist as two
kinds, a skill `foo` beside a bundle `foo`. It costs a segment in every
reference your users type. It also decides how short a reference resolves.
A flat layout resolves from a bare `code-review`, while a segmented one
needs `skills/code-review`.

Decide before your first publish. In practice the choice is one-way,
because a repository path becomes a public reference the moment someone
pins it in a `grimoire.lock`.

## How the index keys a package

An index stores one pointer file per package, at
`index/<host>/<namespace>/<package>/metadata.json`. The `<package>`
directory name must equal the `name` field inside that file.

A package name is claimed by one kind per namespace. The kind lives inside
`metadata.json` rather than in the path. So a skill and a bundle sharing a
name collide at the index, even though they occupy distinct OCI
repositories. `grim publish --announce` refuses a manifest whose entries
share a name, with exit 65.

## What the first-party catalog does

Grimoire's own `catalog/publish.toml` sets `registry = "ghcr.io"` and gives
each entry a per-entry `repository`, which keeps the kind segment. The
skill entry publishes to `grimoire-rs/skills/grim-usage`, and the bundle
entry to `grimoire-rs/bundles/grim-essentials`.

Read that as history, not as advice. Those paths are already published
references, and the section above says a path is fixed once someone pins
it. A catalog starting from scratch is free to go flat.

## Next steps

- [Publishing Skills and Rules](../publishing.md) is the full manifest
  reference.
- [The Package Index](../package-index.md) is the pointer format and the
  ownership rules.
- [Host Your Own Index](../hosting-an-index.md) covers standing the index
  up and gating contributions.
- [Publish a skill to your own index](../tutorials/own-index.md) runs the
  whole path once, end to end.
