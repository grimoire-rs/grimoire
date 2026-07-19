# grim (MCP server)

Run grim as a [Model Context Protocol](https://modelcontextprotocol.io)
server. Installing this artifact registers a `grim mcp` stdio entry in
each detected client's native MCP config — no files are materialized, and
uninstall removes only that entry.

The server exposes the grim catalog and install state to AI agents:

| Tool | Purpose |
|------|---------|
| `grim_search` | Search the configured registries' catalogs |
| `grim_status` | Report declared artifacts and their install state |
| `grim_fetch` | Return artifact content in-context (canonical or per-vendor projection) |
| `grim_describe` | Report manifest-level metadata — kind, curated annotations, tags — without downloading content |
| `grim_render` | Write vendor-native files to a directory (requires `--allow-writes`) |

Scope (`global` / `config` / `workspace`) is chosen per tool call, so one
registration serves every project.

## Install

```sh
grim add ghcr.io/grimoire-rs/mcp/grim:0
```

Requires the `grim` binary on `PATH` —
[installation guide](https://grimoire.rs/installation.html).

## Links

- Documentation: <https://grimoire.rs>
- Source & issues: <https://github.com/grimoire-rs/grimoire>

Published under the Apache-2.0 license.
