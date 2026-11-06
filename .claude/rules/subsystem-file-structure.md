---
paths:
  - src/**
  - crates/**
---

# File Structure Subsystem

How Grimoire lays out its on-disk data: where downloaded artifacts, the
local index, and install links live under the data root. This file is the
core and the index; the install-layout, vendor-path, state and detection
detail lives in the parts listed under Parts.

## Design Rationale

- **Single data root.** All Grimoire state lives under one directory
  (default `~/.grimoire`, overridable via `GRIM_HOME`). Keeping everything
  under one root makes atomic rename / hardlink operations possible because
  source and destination stay on one filesystem.
- **Content-addressed storage.** Downloaded artifacts are addressed by
  content hash so identical content is stored once and is immutable.
- **Mutable namespace on top.** Human-facing names (tags, "installed"
  links) are a thin mutable layer pointing at immutable content.
- **Cross-device safety.** Operations that rely on same-filesystem
  atomic rename must validate the data root sits on a single volume;
  cross-device hardlink/rename fails and must be handled explicitly.

## Parts

Each part is path-scoped to the code it describes and loads when that code
is open. Anchors (`{#...}`) moved with their sections.

| Part | Covers | Anchors |
|---|---|---|
| [subsystem-install-layout-skills-rules.md](./subsystem-install-layout-skills-rules.md) | Install Layout: Skills, Rules | |
| [subsystem-install-layout-agents-mcp.md](./subsystem-install-layout-agents-mcp.md) | Install Layout: MCP servers, Agents | `install-layout-mcp`, `install-layout-agents` |
| [subsystem-install-layout-hooks.md](./subsystem-install-layout-hooks.md) | Install Layout: Hooks | `install-layout-hooks` |
| [subsystem-install-layout-settings.md](./subsystem-install-layout-settings.md) | Install Layout: Settings | `install-layout-settings` |
| [subsystem-vendor-paths.md](./subsystem-vendor-paths.md) | Global-scope paths and vendor environment overrides | `global-scope-paths` |
| [subsystem-client-detection.md](./subsystem-client-detection.md) | Client Detection (default install targets) | `client-detection` |
| [subsystem-install-state.md](./subsystem-install-state.md) | Install State (project, global) and the path containment guard | `install-state`, `install-state-project`, `install-state-global`, `path-containment-guard` |
| [subsystem-path-anchors.md](./subsystem-path-anchors.md) | PathAnchor set and the anchor root/remainder table | `path-anchor-set`, `anchor-remainder-table` |

## Telemetry {#telemetry-installation-id}

`$GRIM_HOME/telemetry/installation-id` holds the opt-in telemetry
installation ID: one lowercase hyphenated UUID v4, written atomically on the
first **armed** run (`GRIM_TELEMETRY` truthy and no disarm) and reused after
that. An unarmed run never creates the file or the directory. A value that
does not parse is replaced, a non-absolute `GRIM_HOME` or any I/O error means
no ID (the export proceeds without `grim.installation.id`), and a shared
`GRIM_HOME` volume shares one ID. **Unstable until 1.0**, not a frozen layout
surface: no migration, no reaper, no upgrade fixture. Not a `ClientOutput`,
so it is invisible to the anchor/remainder table and to reaping. User-facing
contract → `docs/src/content/docs/telemetry.md#installation-id`.


## Catalog Cache {#catalog-cache}

Each browse source (an `oci` or `index` registry entry) caches its listing
under `$GRIM_HOME/catalog/`, one pair of files per source. `<h16>` is the first
16 hex characters of the SHA-256 of the source locator, so sources cache
independently and a refresh of one never clobbers another.

| Path | Owner | Contract |
|---|---|---|
| `$GRIM_HOME/catalog/<h16>.json` | `CatalogFile` | Self-keyed by its `registry` field, so a hash collision rebuilds and never mis-serves. Byte shape unchanged by the sidecar. `deny_unknown_fields` is gone from the five cache structs, so a cache a later grim extended is read, not refused |
| `$GRIM_HOME/catalog/<h16>.sync.json` | SyncState | New, private, versioned, tolerant. Holds the digests, the adopted root's digest and generation, stats digests, `checked_at` and backoff state of the last sync. Stores no URL, locator or userinfo, and its `last_error` is redacted |

The sidecar **binds** to the cache file: it applies only when its `version` is
the one this grim writes, its `locator_sha256` matches the source and its
`catalog_sha256` equals the SHA-256 of the `CatalogFile` bytes on disk.
Anything else ignores it whole and costs one fresh sync, which is how a
mixed-version `$GRIM_HOME` heals. Both files are written with `atomic_write`
under the source's advisory lock, the `CatalogFile` first and the sidecar
second, so a crash between them leaves a digest mismatch and never a half
state. A quiet refresh (an unchanged root or flat digest) writes the sidecar
only and never re-serializes the `CatalogFile`. The uncoordinated fallback
writes the `CatalogFile` alone, which deliberately breaks the binding. Design
and contract: `.agents/specs/design_catalog_at_scale.md` §5.4 and §6.3.

A row's `kind_read: true` (omitted when false) marks an OCI manifest that was
read and names no grimoire kind on any tier, so a refresh reuses that kindless
row unread; a row naming a kind this grim does not know stays unmarked and is
read again. 0.13 and 0.14.x refuse a cache holding the field.

## Constraints

- Never assume a path operation crosses filesystems silently — check first.
- Treat the content store as append-only / immutable; mutate only the
  name → content mapping.
- Concurrent processes must coordinate via advisory file locks for any
  read-modify-write on shared metadata.

## Cross-References

- `arch-principles.md` — overall architecture and utility discipline
- `quality-security.md` — path traversal / symlink-escape guards (two-layer containment pattern)
