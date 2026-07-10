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
| `--format json` reports | The report shape for every command that offers one — see [Additive fields](#frozen-additive-fields) |
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

<!-- internal -->
[status]: ./commands.md#status
[config-exit-codes]: ./commands.md#config-exit-codes
[configuration]: ./configuration.md
[env-vars]: ./configuration.md#environment-variables
[artifacts-kinds]: ./artifacts.md#kinds
[publishing-release]: ./publishing.md#release
[vendor-metadata]: ./vendor-metadata.md
