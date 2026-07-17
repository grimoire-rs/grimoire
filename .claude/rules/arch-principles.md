---
paths:
  - src/**/*.rs
---

# Grimoire Architecture Principles

Auto-loads on every Rust file edit. Provides stable architectural context —
the "why" behind design. For dynamic discovery of current code state,
launch `worker-architecture-explorer`.

The principles below describe shipped structure — the command pattern,
subsystem modules, and ADR-recorded decisions are real code. Update this
file as the architecture evolves.

## Crate Layout

Grimoire is a **single binary crate**:

- Crate / package name: `grimoire`
- Binary name: `grim`
- All source lives under `src/`. No workspace, no separate library crate,
  no lib/CLI split. Acceptance tests live under `test/`.

## Design Principles

These patterns are the intended backbone. Apply them as the codebase grows.

| Principle | Intent |
|-----------|--------|
| **Facade** | A single coordination point hides subsystem complexity from the CLI layer |
| **Strategy / trait dispatch** | Swappable implementations (e.g. local vs remote registry access) for testability |
| **Command pattern** | Uniform CLI flow: args → typed identifiers → operation → report data → output |
| **Three-layer errors** | Top-level error wraps domain errors wraps kinds, so batch operations can diagnose per item |
| **Option-based lookups** | "Not found" is `Option::None`, not an error, at the lookup layer |
| **Extension traits in a prelude** | Ergonomic helpers without polluting core types |
| **Builder pattern** | Fluent construction where there are many optional parameters |
| **Lazily-initialized context** | One init per invocation; avoid unused work |

## Intended Command Flow

```
CLI command (clap parse)
  → Context init (config, registry client, local store)
  → command/{name}.rs — transform args into typed identifiers
    → coordinate the operation (resolve, fetch, install, ...)
  → build report data from results
  → render to stdout (plain / JSON)
```

## ADR Index

Architecture decisions are recorded as `.claude/artifacts/adr_*.md`. Read
the relevant ADRs before making decisions in the same domain.

| ADR | Decision |
|-----|----------|
| [adr_oci_artifact_type.md](../artifacts/adr_oci_artifact_type.md) | Type artifacts with OCI `artifactType` + a Grimoire config media type per kind; retire the `com.grimoire.kind` annotation (superseded by adr_oci_empty_config_compat) |
| [adr_oci_empty_config_compat.md](../artifacts/adr_oci_empty_config_compat.md) | OCI empty config + `com.grimoire.kind` annotation (NO custom `artifactType` — GitLab rejects it); 3-tier kind read (artifactType → legacy config mediaType → annotation) keeps old artifacts readable; supersedes adr_oci_artifact_type |
| [adr_multifile_rules.md](../artifacts/adr_multifile_rules.md) | A rule may carry an optional sibling support directory (`<name>/`) packed into the same single tar layer and installed beside the index `<name>.md`; wire contract unchanged, single-file rules unaffected, install record gains an optional `support_dir` |
| [adr_catalog_summary_annotation.md](../artifacts/adr_catalog_summary_annotation.md) | Add an optional `com.grimoire.summary` annotation, authored in-file for every kind (skill `metadata`, rule frontmatter, bundle `.toml`); keywords are string-only everywhere; `grim search` shows summary-or-description truncated to a terminal-width-clamped window (full when piped), keeps the full description in JSON, and search matches the summary too |
| [adr_tool_namespaced_metadata_rendering.md](../artifacts/adr_tool_namespaced_metadata_rendering.md) | Tool-specific skill capabilities are authored as `<client>.<field>` string keys inside the agentskills `metadata` map; rule vendor-unique keys go in the rule `metadata` map too; common capabilities (e.g. `paths`) stay top-level; grim projects per client at install via per-vendor `Vendor` trait structs (full surface: name, root_dir, skill/rule field registries, scope-aware layout, index transforms, sync_config hook); bad literals hard-fail publish; `claude.*` skill registry in `src/install/vendor_claude.rs`, `copilot.*` rule registry in `src/install/vendor_copilot.rs`; global-scope installs target vendor-native dirs (`~/.claude`, `~/.copilot/skills`, `$XDG_CONFIG_HOME/opencode/skills`), not `$GRIM_HOME` |
| [adr_agent_artifact_kind.md](../artifacts/adr_agent_artifact_kind.md) | Fourth artifact kind `agent`: single `.md`, required frontmatter (`name` == file stem, `description`), common fields `model`/`tools` projected per vendor with a silent `<vendor>.<field>` override escape hatch (`expected_overrides` on `append_lifted`); `--kind agent` required at build/release (`.md` stays rule by shape, agent-shaped rules warn); declaration hash emits `"agents"` only when non-empty (no version bump, lock stays V1 with optional `[[agent]]`); bundles accept agent members; v1 excludes object-valued vendor fields and support dirs |
| [adr_effective_set_mutations.md](../artifacts/adr_effective_set_mutations.md) | Declaration mutations (`remove`/`uninstall`/TUI delete) act on before/after **effective desired sets** instead of surgical lock edits: drop `E_before \ E_after`, keep the intersection with re-derived provenance; the lock caches each declared bundle's expansion in an optional `[[bundle]]` section (binding, repo, tag, digest, member list) so the sets are computable offline; an id-mismatch (surviving holder binds a different identifier than the pinned one) drops the entry and skips the hash restamp — honest staleness over silent omission |
| [adr_repository_annotation.md](../artifacts/adr_repository_annotation.md) | Optional `repository` metadata key (skill/agent `metadata`, rule top-level, bundle TOML) carries an HTTPS source-repo URL emitted as `org.opencontainers.image.source` (spec-correct, ghcr link-back), winning over the tagless release-ref fallback; non-HTTPS values hard-fail publish (65); catalog read-back keeps the annotation only with an `https://` prefix (`CatalogEntry::repository_url`, no version bump); surfaced in the TUI detail pane (`o` opens it) and `grim search` JSON `repository` field |
| [adr_install_state_portability.md](../artifacts/adr_install_state_portability.md) | Project install-state relocates from `$GRIM_HOME/state/projects/<sha>.json` to `<workspace>/.grimoire/state.json` (location is the key — no host-path hash — so it survives a shared `GRIM_HOME`/devcontainer); target paths stored relative to a typed `PathAnchor` (Workspace/ClaudeRoot/CopilotRoot/OpenCodeSkills/OpenCodeRoot/GrimHome) behind a two-layer containment guard (reject non-`Normal`/empty, then canonicalize-and-contain on read; `TraversalAttempt`/`EscapedAnchor` → exit 65 even during prune); `InstallRecord.outputs: Vec<ClientOutput>` replaces the denormalized top-level mirror; on-disk schema V1→V2 (`serde_repr`) with legacy fallback, reap, and a lossy-migration guard; a single `InstallState::persist` seam for all writes; grim self-manages `.grimoire/.gitignore` |
| [adr_git_provenance_annotations.md](../artifacts/adr_git_provenance_annotations.md) | Opt-in `--git` flag on `build`/`release`/`publish` derives provenance from the artifact's git working tree (subprocess, no new crate; `src/oci/git_provenance.rs`) and stamps `org.opencontainers.image.{revision,created,source}`: revision = `HEAD` SHA (`-dirty` suffix for tracked changes), created = the per-commit date (deterministic, NOT wall-clock), source = the `origin` remote normalized to `https://` as a fallback BELOW an authored `repository`. Off by default so an ordinary release stays byte-deterministic/idempotent; with `--git` a re-release from a different commit changes the digest (refused without `--force`). A non-git path / missing `git` is a hard DataError (65). Read-back: additive `CatalogEntry.revision`/`.created` (no `CatalogVersion` bump) → `CatalogRow` → `TuiRow`, surfaced as `Revision:`/`Created:` detail-pane rows and `grim search --format json` `revision`/`created` fields |
| [adr_multi_registry_mcp.md](../artifacts/adr_multi_registry_mcp.md) | One shared `catalog_service::load_catalog` seam (registry-grouped, filtered + badged once) consumed by `search` and the MCP `grim_search` tool; additive `[[registries]]` (`RegistryConfig{alias,url,default}`) in both config scopes with a `resolve_registries` list resolver (legacy `default_registry` folded in only when no `[[registries]]`); qualified refs use the collision-safe `alias/repo` form (never `alias:repo`) via `resolve_reference`, short ids still resolve to default; per-registry cache `$GRIM_HOME/catalog/<hash>.json` (format unchanged, legacy `catalog.json` reaped, no migration) with a generalized `AdvisoryFileLock` + double-checked `load_or_refresh_coordinated` (serve-stale-on-contention, readers never block); `grim mcp` is a `Printable`-exempt local STDIO server on the official `rmcp` SDK, v1 read tools `grim_search` + `grim_status` always on, write tools gated behind `--allow-writes` (launch-pinned scope + the empty write gate were superseded by adr_mcp_percall_scope_fetch_render — per-call scope, `grim_fetch`, `grim_render`); TUI registry-tree projection (collapsible per-registry roots) landed in the issue #16 follow-up (`reload_into` on `load_catalog`; registry-authoritative grouping, precedence order, single-registry elision, empty/offline roots); only the background `spawn_catalog_refresh` seam remains single-registry |
| [adr_mcp_percall_scope_fetch_render.md](../artifacts/adr_mcp_percall_scope_fetch_render.md) | `grim mcp` v2: install scope becomes a per-tool-call parameter trio (`global`/`config`/`workspace`, precedence global > config > workspace-seeded walk-up > cwd walk-up) — deliberate reversal of the launch-pinned scope invariant (local-trust stance: scope/dest params ≡ CLI flags of a user-launched process; real boundaries = registry tar content via `safe_relative_path` + launch-pinned `--allow-writes`); `grim mcp` drops `--global`/`--config` (breaking, exit 64). New tools: `grim_fetch` (use ≠ install — returns canonical or per-vendor projected artifact content in the tool result; 8 MiB pre-download layer gate errors, 256 KiB doc cap truncates with marker) and `grim_render` (vendor-native files to arbitrary `dest_dir`; first write tool, gated via rmcp `ToolRouter::disable_route` — hidden from list AND rejected at call). SSRF stance retained: no registry param on any tool; scope selects which config is read. Offline limitation: manifests not cached, `GRIM_OFFLINE` fetch fails cleanly at manifest |
| [adr_fetch_service_extraction.md](../artifacts/adr_fetch_service_extraction.md) | Break the `command ↔ mcp` fetch cycle by extracting a neutral crate-root `src/fetch.rs` (role-analogue of `catalog_service.rs`): the core takes already-resolved `FetchScope` + `Arc<dyn OciAccess>`, depends on neither `mcp` nor `command`; `command::resolve_fetch_scope` single-sources resolution (forward edge only). Completes CWE-770 hardening on the shared `OciAccess::fetch_blob` seam: a bounded `CappedSink` aborts the streamed body past `max_bytes` → `OversizeBlob`/65, plus a per-caller pre-download policy gate (bundle `BUNDLE_LAYER_SIZE_LIMIT`, fetch `FETCH_BLOB_SIZE_LIMIT`, install `MCP_LAYER_SIZE_LIMIT` for mcp / new `INSTALL_LAYER_SIZE_LIMIT` 512 MiB else, render `INSTALL_LAYER_SIZE_LIMIT`) → `OversizeLayer`/65, making the trait's "verified against policy caps" doc true for every caller. Deferred: typed `FetchError` (fetch core still `anyhow`, so pre-gate oversize exits 1 vs stream 65), registry read/idle timeout. Extends adr_mcp_percall_scope_fetch_render |
| [adr_unified_publish_version_cascade.md](../artifacts/adr_unified_publish_version_cascade.md) | Unify the publish/release version+cascade interface: cascade becomes a tri-state `--[no-]cascade` (default = auto for semver / single tag otherwise; `--cascade` asserts semver → `CascadeRequiresSemver`/65; `--no-cascade` suppresses floats) applied by `publish_tags(tag, cascade)`. Overwrite semantics go **uniform** — exact tag skip-existing by default, `--force` to move, for **every** value incl. channels (the `--tag` always-move special case removed). `grim release` keeps its ref-tag + gains only `--[no-]cascade`; `grim publish` drops `--tag` and makes `--version` the single source (semver → top-level override + cascade; non-semver → uniform channel tag, no cascade; `--cascade`+channel → 65), routed by the `version_mode` classifier. Supersedes publish "D1"/"D3" |
| [adr_grim_config_command.md](../artifacts/adr_grim_config_command.md) | `grim config` — git-style config CLI for **settings + registries only** (declarations stay with `add`/`remove`/`install`/`lock`). Hybrid surface under one umbrella: explicit `config get|set|unset|list <dotted.key>` (e.g. `options.clients`, `options.tui.default_view`, `registry.<alias>.url`) plus **nested** `config registry add|rm|use|show|list`; `registry use` encapsulates the at-most-one-default invariant. `--global` selects `$GRIM_HOME/grimoire.toml`, else project (walk-up / `--config`). Reuses `write_config` (`add.rs`, lossy re-serialize — `toml_edit` upgrade deferred), `scope_resolution`, `ConfigFileLock`; every mutation re-runs `validate_registries` before writing. Migration-script contract: explicit verbs, `--format json`, stable exit codes (unset `get`→1, unknown key→64, bad value→65) |
| [adr_push_pull_registry_split.md](../artifacts/adr_push_pull_registry_split.md) | Push/pull registry split (issue #39): the manifest `registry` stays the canonical PULL name baked into every reference/annotation/report; optional `push_registry` manifest field + `--push-registry host[/prefix]` flag (flag > manifest, on **both** `publish` and `release`) names the network push endpoint only — every network call (push, skip-existing, overwrite guard, tag moves, pin digest resolution, companion push, announce read-back) targets the `Identifier::with_registry`-rewritten id. Pinned bundle members bake pull-named digest-pinned refs resolved via the push endpoint (mirror-correctness trade; foreign-registry members untouched). Reports gain additive always-present `pushed_to` (null when inactive). Unset knob ⇒ byte-identical; malformed value → 65 |
| [adr_codex_vendor.md](../artifacts/adr_codex_vendor.md) | Codex (OpenAI Codex CLI) as a fourth client vendor (`--client codex`): skills install verbatim to the cross-vendor open standard `.agents/skills/` (project workspace, global `$HOME` — **not** `$CODEX_HOME`); agents render to **TOML** at `.codex/agents/<name>.toml` (first TOML vendor — `name`/`description`/`developer_instructions`/optional `model` via the `toml` crate, `#`-comment provenance, `tools` dropped, optional `codex.*` agent registry); **rules are declined** — new `Vendor::supports_kind` gate returns false for `Rule`, installer warns + skips + records zero outputs (Codex has no path-scoped instruction mechanism; hooks rejected upstream); two new `PathAnchor`s `AgentsSkills`/`CodexRoot`; `RenderError::Serialization` keeps the TOML emit panic-free |

## Code Style Conventions

Project-wide conventions enforced by review:

| Convention | Rule | Deviation = Bug |
|------------|------|-----------------|
| **Type names** | Full descriptive names (`OperatingSystem`, `Architecture`), not abbreviations (`Os`, `Arch`) | Abbreviated type names |
| **Module structure** | One concept per file; named module files, no `mod.rs` | Monolithic files, `mod.rs` files |
| **Internal enum exhaustiveness** | Omit `#[non_exhaustive]` on internal non-error enums so matches stay total. The binary is the only consumer — no stable lib API. Error enums are exempt | `#[non_exhaustive]` on a closed internal enum |
| **Domain types over `String`** | Fields representing a domain concept (registry reference, digest, version, platform) use a dedicated type with `Serialize`/`Deserialize` round-tripping through canonical string form, not raw `String` | Stringly-typed domain field |

## Where Features Land

| Feature type | Location | Notes |
|--------------|----------|-------|
| New CLI command | `src/command/` | One file per command, follow the command pattern |
| New output format | `src/api/` | Implement the shared output trait |
| New acceptance test | `test/tests/test_*.py` | Use fixtures, maintain test isolation |

## Utility Discipline

**Before writing a small helper inside a module, check whether `std`,
`tokio`, or an existing crate-level utility already covers it.** A helper
reinvented in one module is wasted effort and a drift risk. If a new helper
is broadly applicable, place it in a shared `utility`/prelude module in the
same change rather than locally. Check `std` first, then existing utilities,
then invent.
