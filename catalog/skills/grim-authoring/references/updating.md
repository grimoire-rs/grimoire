# Updating This Guide

You loaded this file because you maintain the grim-authoring package and
need to refresh its claims against the current grim release.

## Schema Authority

This skill distills, it does not define. The chain of truth, strongest
first:

1. **The installed binary** — `grim build` output and `grim build --help`
   reflect the schema actually compiled in. For the TOML surfaces,
   `grim schema --kind <config|publish|lock|mcp|hook>` emits the schema
   generated from grim's own parse structs, doc comments included — for
   `hook.toml` this is the shortest path to ground truth, shorter than
   reading the source.
2. **The docs site** — [Artifact Reference][artifacts],
   [Vendor-Specific Metadata][vendor], [Publishing][publishing],
   [Agent Artifacts][agents], [MCP Server Artifacts][mcp],
   [Client Compatibility][clients], [The Package Index][index].
3. **The source** — frontmatter structs in [`src/skill/`][src-skill]
   (skill/rule/agent frontmatter, name rules), the hook manifest and its
   validation rules in [`src/oci/hook.rs`][src-hook], and the vendor
   registries in [`src/install/`][src-install] (`vendor_claude.rs` and
   siblings — one file per client). [Client Compatibility][clients] is the
   enforced matrix: a build-time parity test fails if it drifts from those
   files, so it never lies about which client hosts which kind.

## Refresh Protocol

On every grim **minor** release:

1. Re-run `grim build` against this package and the minimal examples in
   each `references/*-spec.md`; fix anything newly rejected or warned.
2. Diff the docs pages above against the field tables and pitfalls
   tables here — new fields, new registries, changed limits.
3. Re-check the client roster against the compatibility matrix: a client
   added, or a `✓`/`◐`/`✗` cell moved, invalidates the per-kind support
   statements in `../SKILL.md`, `rule-spec.md`, `agent-spec.md`,
   `mcp-spec.md`, and `hook-spec.md` at once — those are the first places
   to drift.
4. Re-verify the volatile numbers: name length cap, description cap,
   bundle member/size limits, hook matcher charset/byte cap and default
   `timeout`, enum value sets. Registries grow fastest.
5. **Re-check the hook gate status.** `hook-spec.md` and `../SKILL.md`
   deliberately say hooks ship disarmed and describe no arming behaviour.
   Confirm against the binary before widening either claim: read
   `grim config list --all` for `options.experimental.hooks`, and
   `grim hook list --help` / `grim hook --help` for the user-facing surface.
   `grim hook run` stays documented as launcher-invoked, not a command a
   person types.
6. Bump the `compatibility` frontmatter in `SKILL.md` to the verified
   version line. The prose and the footer stay version-neutral — they
   track the release the package ships beside, so nothing else to bump.

## Durable Search Terms

- `grimoire grim build exit 65 DataError validation`
- `grim vendor metadata projection claude opencode copilot codex cursor gemini registry`
- `grim client compatibility matrix rule agent declined degraded`
- `codex subagents TOML agent developer_instructions`
- `cursor mdc globs alwaysApply` · `kiro steering inclusion fileMatchPattern`
- `gemini subagents experimental.enableAgents`
- `grim catalog metadata summary keywords repository annotation`
- `grim publish announce fork index pull request`
- `grim bundle pin floating members cascade tags`
- `grim hook.toml manifest schema events tiers matcher argv command`
- `grim hook payload not executable registry 0644 interpreter form`
- `grim options.experimental.hooks hook allow workspace consent arming gate`
- `agentskills.io specification metadata map string values`

## Canonical Links

[artifacts]: https://grimoire.rs/artifacts.html
[vendor]: https://grimoire.rs/vendor-metadata.html
[publishing]: https://grimoire.rs/publishing.html
[agents]: https://grimoire.rs/agents.html
[mcp]: https://grimoire.rs/mcp-servers.html
[clients]: https://grimoire.rs/clients.html
[index]: https://grimoire.rs/package-index.html
[src-skill]: https://github.com/grimoire-rs/grimoire/tree/main/src/skill
[src-hook]: https://github.com/grimoire-rs/grimoire/blob/main/src/oci/hook.rs
[src-install]: https://github.com/grimoire-rs/grimoire/tree/main/src/install
