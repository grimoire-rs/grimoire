# grim-authoring

Author, validate, and package grim-publishable artifacts: skill
directories, rule files, agent definitions, MCP server descriptors, and
bundle TOMLs.

The skill carries the per-kind packaging specs — required frontmatter,
catalog metadata, vendor-namespaced keys (`claude.*`, `cursor.*`,
`gemini.*`, … — the vendor namespaces grim reserves), name rules —
plus the release checklist that takes an artifact from `grim build`
validation to a published, announced package.

## Install

```sh
grim add ghcr.io/grimoire-rs/skills/grim-authoring:0
```

## What's inside

- `SKILL.md` — kind chooser and authoring workflow
- `references/skill-spec.md`, `rule-spec.md`, `agent-spec.md`,
  `mcp-spec.md`, `bundle-spec.md` — one packaging spec per kind
- `references/vendor-metadata.md` — vendor-namespaced metadata keys
- `references/release-checklist.md` — build → release → publish → announce
- `references/bootstrap-existing-repo.md` — turning an existing repo into
  a publishable catalog

## Links

- Documentation: <https://grimoire.rs> (Publishing, Artifacts)
- Source & issues: <https://github.com/grimoire-rs/grimoire>

Published under the Apache-2.0 license.
