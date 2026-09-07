---
title: Check an artifact before you install it
description: Read an artifact's contents, provenance, rating and deprecation notice, and pin an exact digest, before anything reaches your agent.
---
<!-- doc_type: how-to -->
<!-- doc_tier: everyday -->

By the end of this guide you have read an artifact's contents, provenance,
rating and deprecation notice. You have also pinned the exact digest you
reviewed, before anything reaches your agent. Only the last step needs a
project with a `grimoire.toml`, which `grim init` writes.

## Read the metadata

1. Ask the registry what the artifact says about itself.

   ```sh
   grim describe ghcr.io/acme/pr-review:1.0.0
   ```

   grim prints a two-column `Key | Value` table of 26 rows and downloads no
   content. Identity, provenance and metadata take 19 of them: `ref`,
   `digest`, `kind`, `name`, `title`, `description`, `has_description`,
   `summary`, `version`, `license`, `repository`, `revision`, `created`,
   `authors`, `vendor`, `url`, `documentation`, `compatibility` and
   `keywords`.

   Four rows carry the publisher's contact channels, `support.issues`,
   `support.chat`, `support.contact` and `support.security`. Two carry the
   deprecation notice, `deprecated` and `replaced_by`. The last row, `tags`,
   lists every published tag, comma joined. Reserved `__grimoire` tags are
   filtered out of it.

2. Take the same rows in machine-readable form.

   ```sh
   grim describe ghcr.io/acme/pr-review:1.0.0 --format json
   ```

   The JSON carries every row above plus the raw `annotations` map.

The `repository`, `revision`, `created` and `authors` rows are the
artifact's provenance. Read the caveat on them at the end of this page
before you lean on any of it.

## Read the content

1. Print the artifact's canonical document without installing it.

   ```sh
   grim fetch ghcr.io/acme/pr-review:1.0.0
   ```

   The contents reach your terminal and nothing reaches your clients.

2. Read what one client would receive instead.

   ```sh
   grim fetch ghcr.io/acme/pr-review:1.0.0 --vendor claude
   ```

   The flag prints that client's projection rather than the document as
   the author wrote it.

This is the step nothing else does for you. A skill body is instructions
your agent will follow, so read it the way you would read a script before
running it.

## Check for a deprecation notice

A publisher marks a version as superseded by writing `metadata.deprecated`
into a skill's frontmatter. `grim describe` reports that text in the
`deprecated` row, and names the successor in `replaced_by` when the
publisher gave one.

A deprecation is information, not a gate. `grim add` on a deprecated
reference prints a `WARN` naming the notice on stderr, and still succeeds
with exit `0`. Nothing blocks the install, so the call stays yours.

## Check what other people thought

1. Sort the catalog by how many upvotes each artifact carries.

   ```sh
   grim search --sort rating
   ```

   Rows come back with the most upvotes first, then by date. Artifacts
   nobody rated sort into a bucket of their own at the end, never as zero
   votes.

2. Add your own vote once you have read the artifact.

   ```sh
   grim rate ghcr.io/acme/pr-review --up
   ```

   A vote goes through the forge the index publishes ratings from. Votes
   are up-only and binary. `--up` registers one and is the default,
   `--remove` retracts your own, and there is no downvote. The credential
   ladder is on the [ratings page](../ratings.md).

## Pin the exact bytes

A tag is a label the publisher can repoint. A digest names bytes, so
declaring the digest you reviewed is what makes the review binding.

1. Capture the digest from the table you read above.

   ```sh
   DIGEST=$(grim describe ghcr.io/acme/pr-review:1.0.0 | awk '$1=="digest"{print $2}')
   ```

2. Declare that digest rather than the tag.

   ```sh
   grim add ghcr.io/acme/pr-review@$DIGEST
   ```

   The declaration in `grimoire.toml` then names exact bytes. A deprecated
   reference still warns on the way in, and still succeeds.

## What grim does not check

grim's mechanism is integrity, not authenticity. The lock pins a manifest
digest, and a pinned artifact installs byte-identical or the install fails.
That proves the content has not changed since you pinned it. It proves
nothing about who wrote it.

The trust decision happens once, at the declaration gesture. `grim add`, an
edit to `grimoire.toml`, or adding a bundle is your statement of trust, and
everything that gesture pulls in inherits it. Every kind is trusted the
same way. A skill, a rule, an agent, an MCP descriptor and a bundle all
reach the same place. There is no per-kind consent prompt.

Four things sit outside that boundary:

- Whether the author is who they claim to be.
- Whether content you trusted is malicious or merely bad.
- A compromised registry account, or a compromised publisher toolchain.
- Anything the harness owns, such as which tools an agent may run and the
  approval prompts it shows at execution time.

grim installs config. It is not the execution policy layer.

Two consequences follow. The provenance rows `grim describe` prints are
annotations the publisher authored, which makes them claims rather than
verified facts. And grim performs no signature verification at all, a gap
the project has weighed and accepted rather than overlooked.

That is why the fetch above matters. Reading the body is the check, and it
is yours to do.

## See it run

<div data-cast="/casts/inspect.cast" data-cast-poster="npt:0:03"></div>
<noscript><a href="/casts/inspect.cast">Download the recording</a></noscript>

## Next steps

Read [Choose how far a reference is allowed to move](./versioning.md) or
[Keep a hand edit across an update](./lifecycle.md).
