# grim-usage

Teach your AI agent to drive the [grim CLI](https://grimoire.rs) — the OCI
package manager for AI skills, rules, agents, MCP servers, and bundles.

The skill covers the full consumer-to-publisher surface: declaring and
installing artifacts (`init`, `add`, `install`, `update`, `status`),
local path sources for in-repo artifacts, registries and scopes, reading
package metadata (`describe`, `fetch`), and the publishing pipeline
(`build`, `release`, `publish`, `login`). A troubleshooting reference maps
exit codes and integrity gates to causes and fixes.

## Install

```sh
grim add ghcr.io/grimoire-rs/skills/grim-usage:0
```

## What's inside

- `SKILL.md` — command map, reference syntax, routing table
- `references/consume.md` — install/update/remove lifecycle, local paths
- `references/publish.md` — build/release/publish, description companions
- `references/registries.md` — registries, scopes, clients, offline mode
- `references/troubleshooting.md` — exit codes, integrity gates

## Links

- Documentation: <https://grimoire.rs>
- Source & issues: <https://github.com/grimoire-rs/grimoire>
- Changelog: `CHANGELOG.md` in this description companion, or the
  [releases page](https://github.com/grimoire-rs/grimoire/releases)

Published under the Apache-2.0 license.
