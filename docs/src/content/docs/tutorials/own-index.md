---
title: "Publish a skill to your own index"
description: "Take one skill from a directory on your disk to a colleague installing it by name from your team's own package index."
---
<!-- doc_type: tutorial -->
<!-- doc_tier: integration -->

By the end of this page, a colleague installs your skill by name from an
index your team owns. The skill starts as one directory of files on your
disk.

## Before you start

Four things must be in place before step 1. Check each one.

- `grim` on your `PATH`. Run `grim --version` to confirm.
- Push access to a namespace on a container registry. This page uses GHCR.
- A GitHub account that can create one repository.
- `npx` on your `PATH`, from [Node.js](https://nodejs.org) 22.14 or above.

## See it run

The recording covers the local half of this page, from `grim init` to the
publish plan.

<div data-cast="/casts/own-index.cast" data-cast-poster="npt:0:03"></div>
<noscript><a href="/casts/own-index.cast">Download the recording</a></noscript>

## Author the skill

1. Create a project directory, change into it, and write a config.

   ```console
   $ grim init
   Path                               Scope    Status
   /home/you/myproject/grimoire.toml  project  created
   ```

2. Write the skill at `skills/hello-review/SKILL.md`. Two frontmatter
   fields are required, and `name` must equal the directory name.

   ```markdown
   ---
   name: hello-review
   description: Give a quick review of a short piece of writing or code. Use when someone asks for a fast sanity check.
   ---

   # Hello Review

   Read what was pasted in. Say one thing that works well, then one
   concrete thing to change.
   ```

3. Validate and pack the skill without pushing it anywhere.

   ```console
   $ grim build ./skills/hello-review
   Kind   Name          Path                   Layer Digest  Status
   skill  hello-review  ./skills/hello-review  sha256:…      built
   ```

   The `built` status means the frontmatter parsed and the layer packed.
   The digest is the content address of that layer, elided here because
   yours is computed from your own file.

## Try it in your agent

1. Declare the skill from its path. `grim add` writes the declaration,
   pins the digest, and installs the artifact in one step.

   ```console
   $ grim add ./skills/hello-review
   Kind   Name          Pinned                          Status
   skill  hello-review  ./skills/hello-review@sha256:…  added
   ```

2. Read the rendered copy grim wrote into your agent. For Claude Code it
   sits at `.claude/skills/hello-review/SKILL.md`. Ask your agent for a
   quick review of a paragraph and watch it load the skill.

3. Confirm the state grim recorded.

   ```console
   $ grim status
   Kind   Name          Source                       Pinned  State
   skill  hello-review  path: ./skills/hello-review  -       installed
   ```

4. Change one line of `skills/hello-review/SKILL.md`, then run
   `grim add ./skills/hello-review` again. The `Pinned` column carries a
   different digest, and the client copy is rewritten from the new source.

   `grim install` on its own reports `unchanged` here and rewrites
   nothing. The declaration pins a content digest, so grim has no new
   content to render until `grim add` pins the edit.

## Publish it

1. Write `publish.toml` beside `grimoire.toml`.

   ```toml
   #:schema https://grimoire.rs/schemas/grim-publish.schema.json
   registry = "ghcr.io"
   repository_prefix = "acme/skills"

   [skills.hello-review]
   version = "0.1.0"
   ```

   Three rules govern those two fields. First, `registry` is a bare
   registry host, such as `ghcr.io`. A value carrying a path fails
   validation with exit 65. Second, the org or team segment is a separate
   field, `repository_prefix`. Third, `repository_prefix` replaces the
   conventional `skills/` segment instead of sitting in front of it.

   So write `acme/skills` to keep the kind segment, and plain `acme` to
   publish flat at `ghcr.io/acme/hello-review`. The quick start passes
   `--registry ghcr.io/acme` for consuming, and that shape is the one most
   people copy into `registry` by mistake.

2. Preview the push plan without touching the registry.

   ```console
   $ grim publish --dry-run
   Kind   Ref                                     Digest    Tags                Status
   skill  ghcr.io/acme/skills/hello-review:0.1.0  sha256:…  0.1.0,0.1,0,latest  dry-run
   ```

   The `Ref` column is the reference your colleague will type. Read it
   before you push, because a published path is hard to change.

3. Store a registry credential. Use a GitHub personal access token with
   the `write:packages` scope.

   ```console
   $ echo "$GITHUB_TOKEN" | grim login ghcr.io -u your-github-login --password-stdin
   Registry  Username           Verification
   ghcr.io   your-github-login  verified
   ```

   `verified` means grim answered the registry's authentication challenge
   with that credential before storing it. A rejected token fails here
   rather than mid-push, and stores nothing.

4. Push the artifact.

   ```console
   $ grim publish
   ```

   The report is the same table as the dry run, with `pushed` in the
   `Status` column and a real digest in the `Digest` column.

## Stand up the index

1. Scaffold an index repository in a fresh directory.

   ```console
   $ npx @grimoire-rs/indexer init acme-index \
       --quick --name acme --title "Acme Index" \
       --base-url https://acme.github.io/index --forge github
   ```

   The scaffolder writes the pointer tree, the site config, the
   contribution gate, and the GitHub Actions workflows.

2. Build the site locally to see what CI will publish.

   ```console
   $ npx @grimoire-rs/indexer build
   ```

   `dist/` holds the rendered catalog page, `all.json`, and a copy of
   every pointer.

3. Create the repository on GitHub, push the scaffold to `main`, and set
   *Settings, Pages, Source* to **GitHub Actions**. The `pages` workflow
   runs and serves the site at your base URL.

## Announce it

1. Point the manifest at your index. Add this table to `publish.toml`.

   ```toml
   [announce]
   repository = "https://github.com/acme/acme-index"
   ```

2. Open the pull request that adds your pointer.

   ```console
   $ grim publish --announce
   announced: https://github.com/acme/acme-index/pull/1
   ```

   The `announced:` line carries the URL of the request grim opened. When
   your credential cannot push to the index repository, grim forks it
   first and opens the request across the two repositories.

   Publish before you announce. The contribution gate accepts a pointer
   only when the registry lists at least one tag for its `ref`.

3. Merge the pull request. The index rebuilds and redeploys, and the new
   pointer appears in `all.json`.

## Install it as a colleague

1. Register the index once per project.

   ```console
   $ grim config registry add acme --index https://acme.github.io/index
   Action          Key            Value                         Scope    Dry Run
   registry-added  registry.acme  https://acme.github.io/index  project  false
   ```

2. Find the skill by name.

   ```console
   $ grim search hello-review
   ```

   The row names the kind, the reference, and the description you wrote in
   the frontmatter.

3. Declare and install it with the full reference. `grim add` writes the
   declaration and renders the artifact in one step.

   ```console
   $ grim add ghcr.io/acme/skills/hello-review
   ```

   The `Pinned` column carries the reference with the digest grim
   resolved, the `Status` column reads `added`, and the skill file appears
   at `.claude/skills/hello-review/SKILL.md`.

   Type the whole reference. The short form `acme/hello-review` fails with
   a parse error, because `acme` is an index alias rather than a registry
   host.

4. Confirm the result.

   ```console
   $ grim status
   ```

   The row names the skill, its registry source, and the state
   `installed`. That is the skill you wrote, installed by name from an
   index your team owns.

## Next steps

- [Publishing Skills and Rules](../publishing.md) covers the manifest
  fields this page skipped.
- [Catalog layout and naming](../guides/catalog-best-practices.md) covers
  repository shape once the catalog grows past one skill.
- [Host Your Own Index](../hosting-an-index.md) covers branding, the
  contribution gate, and private instances.
- [The Package Index](../package-index.md) is the pointer format and the
  announce mechanics in full.
