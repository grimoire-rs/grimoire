---
title: Choose how far a reference is allowed to move
description: Pick one rung of the reference ladder and know exactly what grim update moves at that rung.
---
<!-- doc_type: how-to -->
<!-- doc_tier: everyday -->

By the end of this guide you have picked one rung of the reference ladder
for an artifact. You know exactly what `grim update` moves at that
rung. You need a project with a `grimoire.toml`, which `grim init` writes.

## The ladder

Every reference you declare sits on one of five rungs. They run from the
loosest, which follows anything the publisher pushes, down to the fixed
one, which follows nothing.

- `:latest` follows every release, prereleases excluded.
- `:1` follows `1.x` releases, prereleases excluded.
- `:1.1` follows `1.1.x` releases, prereleases excluded.
- `:1.1.0` names one version.
- `@sha256:<digest>` names one set of bytes.

The first four are all [floating tags](../concepts.md#references-tags-and-digests),
because a tag can be repointed. Only the digest cannot move.

You pick the rung when you declare the artifact. The reference you hand
[`grim add`](../commands.md#add) is the whole choice.

```sh
grim add ghcr.io/acme/test-writer:1
```

## What grim update moves at each rung

`grim update` re-resolves the reference you declared, on every run, at
every rung. Nothing else decides how far a pin may travel.

The table below came from one measured project per rung. Each started at
version 1.1.0, then 1.2.0 and 2.0.0 were published, and `grim update` ran.

| Rung | Declared | What `grim update` did |
|---|---|---|
| every release | `:latest` | Moved to 2.0.0, crossing a major version |
| any `1.x` | `:1` | Moved to 1.2.0, staying inside 1.x |
| any `1.1.x` | `:1.1` | Stayed put until a 1.1.1 existed, then moved to it |
| one version | `:1.1.0` | Stayed put, until the publisher re-released 1.1.0 |
| one digest | `@sha256:<digest>` | Nothing, in every case |

Read the `one version` row twice. A version tag is still a tag, and a publisher
who passes `--force` to `grim release` can repoint it at different bytes.
So four of the five rungs can move under you. Only the digest rung cannot
move.

## When a tag moves under you

A lockfile does not stop a tag from moving. It changes which command
notices.

`grim install` materializes the pin recorded in `grimoire.lock` and
re-resolves nothing. `grim update` re-resolves your declaration and
rewrites that lock. So the lock freezes installs, and update is the command
that unfreezes them.

Publishing cascades tags on the way out. A release of 1.2.3 points `1.2`,
`1` and `latest` at that digest as well. A prerelease such as `1.2.3-rc.1`
is exact-only and moves no floating tag. `grim release` refuses to repoint
an existing exact-version tag at different bytes unless you pass `--force`.

## Pin a digest when nothing may move

1. Read the digest the tag resolves to.

   ```sh
   grim describe ghcr.io/acme/test-writer:1
   ```

2. Declare that digest instead of the tag.

   ```sh
   grim add ghcr.io/acme/test-writer@sha256:<digest>
   ```

The declaration in `grimoire.toml` then names bytes rather than a label. A
digest never moves, so `grim update` has nothing to do for that artifact on
any run.

The cost is the other half of the trade. A pinned digest never picks up a
fix either, and moving it is a decision you make by declaring a new one.

## See it run

<div data-cast="/casts/versioning.cast" data-cast-poster="npt:0:03"></div>
<noscript><a href="/casts/versioning.cast">Download the recording</a></noscript>

## Next steps

Read [Check an artifact before you install it](./inspect.md) or
[Keep a hand edit across an update](./lifecycle.md).
