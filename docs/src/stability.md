# Stability and Versioning

Grimoire is pre-1.0: the CLI, formats, and OCI pipeline documented across
this book are real and tested, but the surface has moved between minor
releases while the project found its shape.

1.0 draws a line. A script parsing [`grim status --format
json`][status], a shell `case` on an [exit code][config-exit-codes], or a
tool reading `grimoire.lock` needs to know what survives an upgrade
unmodified and what does not — otherwise "just run `grim update`" is a
gamble, not a routine operation.

This page names exactly what becomes a semver-guarded contract at 1.0 and
what is explicitly excluded from it.

## Frozen at 1.0 {#frozen}

Breaking any guarantee below is a major-version change, not a minor one.

| Area | Guarantee |
|------|-----------|
| CLI surface | Subcommand names, arguments, flags, and [documented exit codes][config-exit-codes] |
| `--format json` reports | The report shape for every command that offers one, and the [error document][json-interface] — see [Additive fields](#frozen-additive-fields) and the [JSON interface reference][json-interface] |
| `grimoire.toml` / `grimoire.lock` | The [config and lock schema][configuration] |
| Install state (`state.json`) | Schema V2, governed by the same additive-field policy as JSON reports |
| OCI wire format | [Artifact kinds][artifacts-kinds] and the [release/push mechanics][publishing-release] |
| Environment variables | The documented [`GRIM_*` set and honored vendor overrides][env-vars] |

### Additive fields {#frozen-additive-fields}

Two rows above — `--format json` reports and the install-state schema —
share one rule: a minor release may add a new optional field, but never
changes an existing field's type or meaning, and never removes one. The
matching obligation sits on the reader: a consumer of either format must
ignore fields it does not recognize rather than error on them. That pairing
is what makes "add a field in a minor" safe for every consumer, including
ones written before the field existed.

Optional report fields are **always present**: a field that does not apply
serializes as an explicit `null`, never as an absent key. A consumer can
therefore distinguish "not applicable" (`null`) from "talking to an older
grim that predates the field" (key missing) without version sniffing.

## Unstable — may change in any minor {#unstable}

Two things are deliberately excluded from the guarantee above, because
freezing them would block improving Grimoire's on-disk footprint without a
major version bump:

- **Vendor render layout.** The exact files and paths grim writes under
  `~/.claude`, `.claude/`, `~/.copilot`, the OpenCode config directories,
  and where an MCP entry lands inside a client's own config file are not a
  contract. They are an implementation detail of the [vendor projection
  layer][vendor-metadata], free to move between minors as clients change
  their own conventions.
- **Everything else that is not exit codes or JSON.** State-file contents
  beyond the schema guarantee, TUI appearance and keybindings, and
  human-readable log or error text carry no compatibility promise — only
  exit codes and structured JSON output are contracts.
- **NDJSON progress events** (`--progress json`) are **experimental
  pre-1.0**: the event shapes evolve additively only (new fields may
  appear, existing ones keep their meaning), and the surface freezes at
  1.0 once external consumers have validated it.

### The supported discovery channel {#unstable-discovery}

Because render layout can move, scripting "where did grim put this skill?"
against a hardcoded path is unsupported and will eventually break. Use
[`grim status --format json`][status] instead: every entry carries an
`outputs` array of `{client, path}` pairs — the per-client materialized
locations read back from install state, empty for a declared-but-not-yet-
installed artifact. `outputs` is itself covered by the [additive-field
policy](#frozen-additive-fields) above, so code that reads it survives an
upgrade even as the paths inside it change.

## The compatibility promise {#promise}

Vendor layout moving is not, by itself, a compatibility break — provided
grim upholds this: artifacts remain discoverable by the target client;
status, update, and uninstall keep working across upgrades; exact vendor
paths may change in a minor release with automatic migration.

The reasoning for keeping render layout out of the 1.0 contract while still
holding that promise is recorded in the project's ADR on render-layout
stability (`.claude/artifacts/adr_render_layout_stability.md`).

## Known limitations {#limitations}

Two behaviors fall outside every guarantee above — not because they are
likely to change, but because they are hard constraints of the current
design.

### Forward compatibility {#limitations-forward-compat}

Every lock and install-state field parses with `deny_unknown_fields`: a
`grim` binary that does not recognise a field refuses to load the file
rather than silently drop it. That protects a downgrade from misreading
data it cannot faithfully represent, but it cuts both ways — a lock or
state entry using [local path sources](./concepts.md#references-tags-and-digests)
(a path-declared skill, rule, or agent, or a [local
bundle](./concepts.md#bundles)) is unreadable by a `grim` build that
predates the feature. It exits 78 (`EX_CONFIG`), the same code any other
config or lock parse failure uses.

This only triggers when the feature is actually in use: a registry-only
lock or state file stays byte-identical across the version boundary, so a
project that never declares a path source is unaffected either way.

### Offline re-materialization needs a manifest {#limitations-offline-remat}

Grimoire caches a fetched artifact's content layer — content-addressed, so
identical bytes are never re-downloaded — but not its manifest. An offline
[`grim install`][install] whose rendered output is still on disk is
network-free: the integrity gate compares the on-disk content hash against
the lock and needs nothing from the registry.

Deleting that output and asking `--offline` to re-materialize it is a
different story. Even a pinned manifest digest has to be *fetched* to learn
which layer blob to pull, and that fetch always needs the network — grim
keeps no local manifest cache to serve it from. This is a general
constraint of the content-cache design, not specific to path sources: it
applies to every registry-sourced kind (skill, rule, agent, MCP server, or
bundle member) whose materialized output has gone missing while offline.
[Local path sources](./concepts.md#references-tags-and-digests) are
unaffected — they read straight from disk and never touch a manifest.

<!-- internal -->
[json-interface]: ./json-interface.md
[status]: ./commands.md#status
[install]: ./commands.md#install
[config-exit-codes]: ./commands.md#config-exit-codes
[configuration]: ./configuration.md
[env-vars]: ./configuration.md#environment-variables
[artifacts-kinds]: ./artifacts.md#kinds
[publishing-release]: ./publishing.md#release
[vendor-metadata]: ./vendor-metadata.md
