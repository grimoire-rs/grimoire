# grim-usage

Teach your AI agent to drive the [grim CLI](https://grimoire.rs) — the OCI
package manager for AI skills, rules, agents, MCP servers, hooks, and bundles.

The skill covers the full consumer-to-publisher surface: declaring and
installing artifacts (`init`, `add`, `install`, `update`, `status`),
local path sources for in-repo artifacts, registries and scopes, reading
package metadata (`describe`, `fetch`), and the publishing pipeline
(`build`, `release`, `publish`, `login`). It also covers exporting plugins and
serving a plugin marketplace from a git repository, adding one in Claude Code,
Copilot, Codex, Qoder or Cursor with no grim installed, and the opt-in
telemetry switches. A troubleshooting reference maps exit codes and integrity
gates to causes and fixes.

## Install

```sh
grim add ghcr.io/grimoire-rs/skills/grim-usage:0
```

## What's inside

- `SKILL.md` — command map, reference syntax, routing table
- `references/consume.md` — install/update/remove lifecycle, local paths
- `references/publish.md` — build/release/publish, compression, plugin export
- `references/marketplaces.md` — running `export marketplace`; adding, pinning,
  rolling out and troubleshooting a grim-generated marketplace
- `references/telemetry.md` — enabling, proving off, and auditing telemetry
- `references/registries.md` — registries, scopes, clients, offline mode
- `references/troubleshooting.md` — exit codes, integrity gates

## Links

- Documentation: <https://grimoire.rs>
- Source & issues: <https://github.com/grimoire-rs/grimoire>
- Changelog: `CHANGELOG.md` in this description companion, or the
  [releases page](https://github.com/grimoire-rs/grimoire/releases)

Published under the Apache-2.0 license.
