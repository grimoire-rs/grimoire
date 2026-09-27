# Plan: share a project as a plugin

## Status

- **Plan:** plan_export_project
- **Active phase:** 1 — Implementation
- **Step:** awaiting /hex-review
- **Last update:** 2026-09-27 (implemented on hex/harness-native-marketplaces; task verify green)
- State:   implemented
- Tier:    medium

## Context

Sharing "what my project runs" as a harness plugin meant hand-extracting
pins from `grimoire.lock`. Owner asked for a thin convenience wrapper, plugin
metadata in `grimoire.toml`, and a marketplace entry pointing at a project.
Decisions (owner, 2026-09-27): pins come from the project's own lock; the
name is required (`--name` or `[plugin].name`, else 64); the four scalar
`[plugin]` fields are `grim config` keys. Recorded as an amendment to
`.agents/adr/adr_harness_plugin_export.md`.

## Design

1. `[plugin]` in `grimoire.toml` — `config::plugin_meta::PluginMeta`
   (`name`, `description`, `version`, `logo`, `rename`), validated at load
   (78), outside the declaration hash. Shared grammars moved out of
   `export::marketplace`.
2. `grim export plugin --project` — `ExportMode::FromLock`; the lock via
   `install::fresh_lock`; members straight into `plugin_input`; project dir
   as the path-source anchor and drift hint.
3. `project = "<dir>"` in `marketplace.toml` — exclusive with `include`;
   `ProjectLock::load`; per-field metadata merge (`merged_decl`); excluded
   from `marketplace.lock`, `declaration_hashes` and `resolve_marketplace`
   (`MarketplaceManifest::include_plugins`); `grim update --marketplace`
   skips them and refuses a selector naming one (64).
4. `grim config get|set|unset|list` rows for `plugin.name|description|version|logo`.

## Tests

- Unit: `config::plugin_meta` rules; `[plugin]` parse + hash invariance;
  include/project exclusivity; project plugins outside the hashes; clap
  conflicts for `--project`; project-dir drift hint.
- Acceptance: `test_export_plugin.py` S-035 (from-lock pins without
  resolving, metadata + overrides, 64/65/79, `--config`, drift, schema) and
  S-036 (project plugin pins + metadata merge, stale 65, update refusal);
  `grim config` key round-trips.

## Verification

`task verify`, `task docs:check`, `task catalog:verify`.
