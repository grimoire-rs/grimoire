---
name: support-desk
description: Route a question to the team that owns an artifact. Use when a published skill misbehaves and you need the maintainer, the issue tracker, or the security contact rather than a code change.
license: MIT
compatibility: claude>=2
metadata:
  summary: Maintainer-routing skill (annotation demo)
  keywords: support,contact,maintainer,demo
  repository: https://github.com/grimoire-rs/grimoire
  authors: Grimoire Platform Team
  vendor: Grimoire Manual Rig
  homepage: https://grimoire.rs
  documentation: https://grimoire.rs/publishing.html#metadata-descriptive
---

# Support Desk

The rig's **annotation showcase**. Every optional metadata field grim knows
how to publish is authored here or in `catalog/publish.toml`, so one artifact
exercises the whole read path at once:

- Frontmatter `license`, `compatibility`, and the descriptive `metadata` keys
  (`authors`, `vendor`, `homepage`, `documentation`) become
  `org.opencontainers.image.*` / `com.grimoire.*` annotations on the manifest.
- `revision` and `created` are derived from the publishing commit — no flag
  needed, and no wall clock is read, so re-publishing the same commit yields
  the same digest.
- The repository's **support channels** ride the description companion, not
  this manifest, so moving a chat link is a `grim publish` re-run rather than
  a re-release of every version.

See scenario 9 in the rig README for what each surface shows.
