---
title: "One directory every agent reads"
description: "Commit one directory of skills and rules every contributor's agent reads, plus a machine-wide set."
---
<!-- doc_type: how-to -->
<!-- doc_tier: everyday -->

This guide keeps a repository's skills and rules in one committed directory every agent reads, plus a machine-wide set installed from a registry. It assumes `grim` is on your `PATH` and a repository you can commit to.

## The repository's own directory

A skill worth writing is worth using from whichever agent you happen to open. The usual outcome is a copy under each client's directory, edited in one of them, and three versions of the same instructions a month later.

Keep one copy in the repository instead. Put the sources somewhere neutral, declare them as path sources, and let grim render a copy for each client on the list.

```text
agent-config/
  code-review/
    SKILL.md
  repo-conventions.md
```

The shape of each source decides its kind. A skill is a directory holding `SKILL.md`, and a rule is a single `.md` file. A directory holding one loose `.md` file is neither, and `grim add` refuses it with exit 64:

```text
cannot infer a kind for './agent-config/house-rules': expected a skill directory (SKILL.md) or a rule .md file; pass --kind agent for an agent
```

Exit `64` is the usage code, and this is the path-source case. The same
failure against a registry reference exits `65` instead, which
[keep a company registry beside the public one](./registries.md) covers.

1. Create the project config.

   ```sh
   grim init
   ```

2. Name the clients this repository renders for.

   ```sh
   grim config set options.clients claude,codex,cursor
   ```

3. Declare the skill by its directory.

   ```sh
   grim add ./agent-config/code-review
   ```

4. Declare the rule by its file.

   ```sh
   grim add ./agent-config/repo-conventions.md
   ```

5. Look at what the two declarations wrote.

   ```sh
   find .claude .cursor .agents -type f | sort
   ```

Two sources, five files, each in the shape its client reads:

```text
.agents/skills/code-review/SKILL.md
.claude/rules/repo-conventions.md
.claude/skills/code-review/SKILL.md
.cursor/rules/repo-conventions.mdc
.cursor/skills/code-review/SKILL.md
```

Three skill copies and two rule files. One of them even changed extension on the way in. The missing sixth file is the subject of the last section.

## Commit the declaration, not the copies

What travels with the repository is `agent-config/`, `grimoire.toml` and `grimoire.lock`. The five rendered files are output, so ignore them:

```gitignore
.agents/
.claude/
.cursor/
.grimoire/
```

Narrow those entries when the repository commits other files under the same directories. With them in place, `git status --short` on a first commit shows the sources and the declaration and nothing else:

```text
A  .gitignore
A  agent-config/code-review/SKILL.md
A  agent-config/repo-conventions.md
A  grimoire.lock
A  grimoire.toml
```

`grimoire.lock` is what makes the copies reproducible. It records a content hash per source, so a rendered file that drifts from it is visible to [`grim status`](../commands.md#status):

```toml
[[skill]]
name = "code-review"
path = "./agent-config/code-review"
hash = "sha256:df8e050e7de6240067350d2c57958e6007b487c966cb4e713beeedc9261d37ea"

[[rule]]
name = "repo-conventions"
path = "./agent-config/repo-conventions.md"
hash = "sha256:51e8e07133c0840733e4c9c11c0f8cca05aa54f89135a468a0b40ea2860b7c6f"
```

A contributor who clones the repository runs one command and gets all five files:

```sh
grim install
```

Nobody edits a rendered copy. Edits go to `agent-config/`, and `grim update` re-renders from there.

## The same set on every machine

The second half of the problem is the set that is yours rather than the repository's. Take adversarial review: one agent reviews a diff the other agent wrote, under instructions that have to be the same instructions. Two hand-copied files with the same name are exactly what that job cannot survive.

Project scope does not help here, because the artifact is not tied to one checkout. Install it at global scope from a registry, so the same reference resolves from any directory on the machine.

1. Declare it at global scope without rendering it yet. [`grim add`](../commands.md#add) has no `--client` flag, so `--no-install` is how you declare first and pick targets after.

   ```sh
   grim add --global --no-install ghcr.io/michael-herwig/arcana/nox-review
   ```

2. Render it for both agents.

   ```sh
   grim install --global --client claude,codex
   ```

3. Read both copies off the disk.

   ```sh
   ls ~/.claude/skills/nox-review ~/.agents/skills/nox-review
   ```

The result table prints one row and names one target, so step 3 is not optional. There is no single directory both agents read here. What there is instead is one lock entry rendering two byte-identical copies, and one [`grim update --global`](../commands.md#update) rolling both forward together.

## When a client declines a kind

Not every client has a home for every kind. When one on your list has nowhere to put a rule, grim writes no file for it and tells you nothing. The run exits `0`, prints its one `added` row, and emits no warning:

```text
Kind  Name              Pinned                                                                                                      Status
rule  repo-conventions  ./agent-config/repo-conventions.md@sha256:51e8e07133c0840733e4c9c11c0f8cca05aa54f89135a468a0b40ea2860b7c6f  added
```

The absent file is the whole signal, which is why step 5 above lists the disk. `grim status --format json` is the other way to see it. Its `outputs` array names one rendered file per client, and a client with nowhere to put the kind is not in that array. Nothing lands in `clients_unresolved`, which reports a different problem.

Which client takes which kind lives in the [client compatibility matrix](../clients.md), one cell per client and kind. That table is generated from the code that performs the install, so read the answer there rather than from a summary anywhere else.

## See it run

<div data-cast="/casts/shared-skills.cast" data-cast-poster="npt:0:03"></div>
<noscript><a href="/casts/shared-skills.cast">Download the recording</a></noscript>

## Next steps

- [Choose a scope and choose your clients](./scopes-and-clients.md) covers the project and global split in its own right, including what a dropped client leaves behind.
- [The client compatibility matrix](../clients.md) is where you check a kind before you add a client to the list.
- [Share a lock with your team](./team-ci.md) turns the committed lock into a CI job that fails when a checkout has drifted.
