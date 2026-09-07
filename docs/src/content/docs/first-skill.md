---
title: Write your first skill
description: Write a minimal SKILL.md, install it from its path, and see an agent load it.
---
<!-- doc_type: how-to -->
<!-- doc_tier: first-steps -->

Write one skill by hand, install it from the path it sits at, and watch an
agent pick it up. Nothing here is published or fetched over the network. The
whole loop stays inside one repository.

## See it run

<div data-cast="/casts/first-skill.cast" data-cast-poster="npt:0:03"></div>
<noscript><a href="/casts/first-skill.cast">Download the recording</a></noscript>

## Write the file

A skill is a directory with a `SKILL.md` inside it. The frontmatter carries
two keys. `name` is the identity the agent calls it by. `description` is what
the agent reads to decide whether to load the skill, so write it as a trigger
rather than as a summary.

```text
skills/
└── hello-world/
    └── SKILL.md
```

That one file is the whole artifact:

```markdown
---
name: hello-world
description: Use when someone asks for a hello-world smoke test of the skill pipeline.
---

# Hello world

Reply with `hello from hello-world` and nothing else.
```

## Check it builds

`grim build` reads the directory and validates it in place:

```sh
grim build skills/hello-world
```

A skill that passes prints one table row ending in `built`. That row is your
first visible result, and it means the frontmatter parsed.

## Install it from its path

`grim add` accepts a local path as readily as a remote reference. Point it at
the directory you just built:

```sh
grim add ./skills/hello-world
```

That declares a path source and pins it by content hash. `grim status` shows
it as `path: ./skills/hello-world`.

In a project that already has a `.claude` directory, the skill lands at
`.claude/skills/hello-world/SKILL.md`. A project with no client marker
directory lands somewhere else, which the
[quick start](./quickstart.md) covers.

## See the agent load it

Three things prove it worked, in order.

1. The file is on disk at its client path.

   ```sh
   cat .claude/skills/hello-world/SKILL.md
   ```

2. `grim status` lists the binding with its path source.

   ```sh
   grim status
   ```

3. Start a fresh agent session and ask it to use `hello-world` by name. It
   should reply with the one line the file tells it to.

Nothing signals a running agent that a file appeared, so a session that was
already open will not see the skill. Start a new one, or restart the one you
have.

## When the frontmatter is wrong

`grim build` exits `65` when the frontmatter does not parse, and it names both
the file and the problem:

```text
skills/hello-world/SKILL.md: invalid YAML frontmatter: missing field `description`
```

Two more mistakes land on the same exit code. A name like `bad skill` is
rejected, because a skill name may hold only lowercase letters, digits,
hyphens, and periods. A file with no closing `---` reports
`missing YAML frontmatter` instead.

The exit code is how a script detects the failure. A wrapper checks for `65`
rather than matching on the message text, which is free to change.

## Next steps

Once the skill is worth more than one repository,
[publishing](./publishing.md) covers packaging it and shipping it to a team.
To install it for every project on the machine instead of this one, read
[scopes and clients](./guides/scopes-and-clients.md).
