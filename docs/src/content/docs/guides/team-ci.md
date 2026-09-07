---
title: Pin a team and gate CI
description: Lock a shared skill set for your team and make CI fail the moment the installed tree drifts from that lock.
---
<!-- doc_type: how-to -->
<!-- doc_tier: integration -->

This page locks a shared skill set for your team and commits it to the
repository. Then it runs the same install in CI so the build fails when the
tree drifts from the lock.

## Before you start

You need a repository your team clones, and `grim` on the machine you run
these commands from. [Installation](../installation.md) covers putting the
binary in place. You also need a registry that every developer and the CI
runner can pull from.

## Declare and lock the shared set

1. Create the project declaration.

   ```sh
   grim init
   ```

2. Add each shared artifact at the tag rung the team tracks.

   ```sh
   grim add ghcr.io/acme/skills/code-review:1
   ```

   One `grim add` writes the declaration and resolves the pin:

   ```text
   Kind   Name         Pinned                                    Status
   skill  code-review  ghcr.io/acme/skills/code-review@sha256:…  added
   ```

3. Read back what the lock holds.

   ```sh
   cat grimoire.lock
   ```

   ```toml
   rule = []
   agent = []

   [metadata]
   lock_version = 1
   declaration_hash_version = 1
   declaration_hash = "sha256:…"
   generated_by = "grim 0.14.1"
   generated_at = "2026-09-07T03:02:30Z"

   [[skill]]
   name = "code-review"
   pinned = "ghcr.io/acme/skills/code-review@sha256:…"
   ```

The `pinned` field is a digest, not a tag, so every clone of the repository
resolves the same bytes. The `declaration_hash` field under `[metadata]` is a
hash of the declarations the lock was built from. That one field is what makes
drift detectable, and the CI gate below rests on it. The full field list is at
[`grimoire.lock`](../configuration.md#grimoire-lock).

Digests and hashes are shown elided on this page. The recording at the foot of
the page prints them in full, and your own run prints yours.

## Commit the lock

Two files go into version control, and nothing else:

```sh
git add grimoire.toml grimoire.lock
```

`grimoire.toml` is the file your team edits. `grimoire.lock` is machine-owned,
and committing it is what makes six developers install the same digests.

You do not have to touch your root `.gitignore`. The first time grim creates
`.grimoire/`, the directory that holds project install state, it writes
`.grimoire/.gitignore` with a single `*` in it.

## Run the same install in CI

The job needs three steps: check the repository out, put `grim` on the runner,
and install from the lock.

```yaml
# .github/workflows/agent-config.yml
name: agent config
on: [push, pull_request]

jobs:
  agent-config:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: grimoire-rs/setup-grimoire@v1
      - run: grim install
```

Use a plain online `grim install` here, not `--offline`. Locking caches the
digest a tag resolved to, not the artifact content, so `--offline` against a
cold runner cache exits `81`.

## Make CI fail when the tree diverges

This is the gate. A hand edit to `grimoire.toml`, or a merge that brings in a
declaration nobody locked, leaves the stored `declaration_hash` behind the live
config. `grim install` refuses and writes nothing at all:

```text
$ grim install
grimoire.lock is stale (declaration_hash sha256:… does not match current sha256:…); run `grim lock` before installing
$ echo $?
65
```

The first hash is the `declaration_hash` from the lock you printed above. The
second is the hash of the declarations as they stand in `grimoire.toml`, and the
two differing is the whole signal.

Exit `65` fails the job, and the refusal lands before the first artifact is
touched. Under `--format json` the same run emits an error document carrying
the same message and the same code.

`grim status --check` is not the gate. It sees the same divergence and reports
it as `stale` on every row, and it still exits `0`:

```text
$ grim status --check
Kind   Name         Source  Pinned                                    State
skill  code-review  direct  ghcr.io/acme/skills/code-review@sha256:…  stale
skill  lint-rules   direct  -                                         stale
$ echo $?
0
```

The reference statement for both commands sits under
[`grim install`](../commands.md#install).

A missing lock is the neighbouring failure, with its own code and its own
message. `grim install` exits `79` and names the path it looked at:

```text
$ grim install
no grimoire.lock found at /path/to/project/grimoire.lock; run `grim lock` first
$ echo $?
79
```

Either way the cure is [`grim lock`](../commands.md#lock), which re-resolves
the declarations and rewrites the lock. The install runs clean after it:

```text
$ grim lock
Kind   Name         Pinned                                    Action
skill  code-review  ghcr.io/acme/skills/code-review@sha256:…  unchanged
skill  lint-rules   ghcr.io/acme/skills/lint-rules@sha256:…   locked
$ grim install
Kind   Name         Target                      Status
skill  code-review  .claude/skills/code-review  unchanged
skill  lint-rules   .claude/skills/lint-rules   installed
$ echo $?
0
```

## Confirm the install is frozen

A moved tag does not move what CI installs. Publishing `1.0.1` repoints the
floating `1` tag at a different digest, and the next `grim install` still
reports `unchanged` against the digest in the lock.

To learn that a tag has moved without moving anything, read the check report as
JSON:

```sh
grim status --check --format json
```

A row whose tag has moved carries `"update_available": true`, and a row with
nothing behind it carries `false`. Moving to the new pin is
[`grim update`](../commands.md#update), followed by a commit of the rewritten
lock. Every state word these tables print is defined at
[artifact states](../commands.md#artifact-states).

## See it run

<div data-cast="/casts/team-ci.cast" data-cast-poster="npt:0:03"></div>
<noscript><a href="/casts/team-ci.cast">Download the recording</a></noscript>

## Next steps

- [Track a version rung](./versioning.md): which rung to declare, and what
  `grim update` moves.
- [Add a company registry beside the public one](./registries.md): pulling the
  shared set from a source your company runs.
- [Concepts](../concepts.md): how the declaration, the lock and the install
  record relate.
