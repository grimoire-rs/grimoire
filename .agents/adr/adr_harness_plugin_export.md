# ADR: Harness plugin export — `marketplace.toml`/`marketplace.lock`, `grim export plugin`, `grim update --marketplace`

## Metadata

**Status:** Accepted — accepted by the autonomous goal loop after the plan review panel and cross-model review; amendments A1–A20
**Date:** 2026-09-27
**Deciders:** Owner (ratified discussion), Architect (hex-plan xhigh)
**Beads Issue:** N/A
**Related PRD:** `.agents/discussions/harness-native-marketplaces.md` (ratified; phase 1 only), goal `.agents/goals/harness-native-marketplaces.md`
**Tech Strategy Alignment:**
- [x] Decision follows Golden Path in `.claude/rules/product-tech-strategy.md` (Rust 2024, Tokio; one new crate `zip`, justified in D8)
**Domain Tags:** api | integration | data
**Supersedes:** N/A
**Superseded By:** N/A
**Companion spec:** `.agents/specs/design_harness_plugin_export.md` (contracts C-001…C-035, scenarios S-001…S-030)

## Context

Phase 1 of the ratified discussion: let an admin hand a team a harness-native
plugin (Claude shape, Agent Plugins 1.0) with no `grim` on the consumer
machine. Every owner decision in `Decisions` is binding and not relitigated
here — in particular the **Reuse mandate**: no second lock, update or render
path; new code only for the two `plugin.json` shapes, zip writing,
`marketplace.toml` parsing and the `rename` step. This ADR fixes the frozen
contracts (schemas, CLI, JSON, exit codes) before any code (goal criterion 1).

Seams verified in code (file:line in the spec): resolver `resolve_lock` /
`resolve_lock_partial` take a `DesiredSet` and assume `(kind, name)`
uniqueness, and a direct `[skills]` entry wins over a bundle member only for
*registry* entries (`direct_keys`, `resolver.rs:289`; path entries are pinned
separately); `lock_io::save`/`content_equal` sort by `name`; the grimoire-lock
JSON schema is generated from `RawLock` and URL-frozen;
`ClientTarget::materialize` renders one member into a caller-chosen `dest`;
`json_splice::upsert_member("", …)` builds a fresh MCP config file;
`ClaudeVendor::mcp_entry` ignores scope.

## Decision Drivers

Weights used in every option table: **R** reuse / no duplicate path (×3),
**P** Principle 9 additivity + reversibility (×3), **S** simplicity (×2),
**U** fit for the admin/team user (×2). Scores 1–3, max 30.

## Industry Context & Research

- `research_marketplace_lock_patterns.md` — one lock with a scope key beside
  each entry (pnpm `importers`, Bun `workspaces`) is universal; unknown
  partial-update selector = hard error everywhere; Claude Code compares plugin
  `version` by **string equality**; 12-hex content suffix (Go pseudo-versions);
  Helm/vsce retro-fitted reproducibility — build it in day one.
- `research_plugin_manifest_fields.md` — Agent Plugins root is
  `additionalProperties:false` with required `$schema` const; Claude strips
  unknown keys; `<v>+<hash>` accepted by both; MCP field mapping and declines.
- `research_reproducible_zip_rust.md` — `zip` 8.6, Stored, DOS-epoch time,
  0o644, sorted, no dir entries, no extra fields.
- `research_plugin_support_matrix.md`, `research_agent_plugins_spec_verify.md`,
  `research_claude_app_install_surfaces.md` — family membership, zip upload is
  Anthropic-only, `.claude-plugin/plugin.json` at archive root.

**Key insight:** every consumer treats the plugin `version` as an opaque
string, so `<v>+<content-hash>` is a correct update signal *because* nobody
compares semver precedence. Recorded as a load-bearing assumption (Risks).

## Decisions

### D1 — `marketplace.toml` schema

```toml
[plugins.team]                       # key = plugin name (SkillName grammar)
include = ["ghcr.io/acme/stack:1", "acme/review:2", "./skills/local"]
description = "Team skills"          # optional
version = "1.4.0"                    # optional; semver, no build metadata; leading v stripped
[plugins.team.rename]                # optional table
strip_prefix = "team-"               # required inside the table; non-empty
```

| Option | R | P | S | U | Σ | Note |
|---|---|---|---|---|---|---|
| **a. Phase-1 minimal, `deny_unknown_fields`; top-level `name`/`owner`/`description` reserved (rejected until phase 2)** | 3 | 3 | 3 | 2 | **28** | Widening later is additive input (stability.md mirror rule) |
| b. Accept top-level fields now, unused | 3 | 2 | 2 | 2 | 23 | Freezes `owner`'s type before phase 2 knows it (Claude wants an object) |
| c. Lenient parse (unknown keys ignored) | 3 | 1 | 3 | 1 | 20 | Typos silently change what ships; contradicts publish.toml precedent |

**Chosen: a.** Discovery `--marketplace <path>`, default `./marketplace.toml`,
no walk-up (publish.toml precedent). The path must end in `.toml` and must not
be named `grimoire.toml` (65), so the data lock `<stem>.lock` never collides
with the manifest, with `grimoire.lock`, or with the advisory sidecar
`<file>.lock`. Read with `config::read_capped`; every load/parse/validation
failure → 65 (publish.toml `load_manifest` precedent).
Plugin names (table keys, `--name`, derived) must parse as `SkillName`
(lowercase alnum, single `-`/`.` separators, ≤64) — a strict subset of both the
Claude rule (no space/`@`/`:`/`/`) and the Agent Plugins regex. `include`:
non-empty; any form `grim add <reference>` accepts (fully qualified, short id,
alias-qualified, local path), any kind. Short ids, aliases and relative paths
resolve against the registry context and directory of the **manifest**
(`resolve_fetch_scope(…, workspace = M.parent())`), not the cwd, so the lock
and its hashes do not depend on where grim runs. Reversibility: **two-way**
(every widening is additive).

**JSON schema (`grim schema --kind marketplace`)**: options — ship now (new
frozen URL, `test_check_urls.py` edit, schemars derive: Σ 21), defer (Σ 27),
ship only a file-local comment (Σ 22). **Chosen: defer** — adding a schema id
later is additive; shipping one freezes a URL for a file grim will soon edit
itself (Decisions › Plugin sources: machine-editable).

### D2 — `marketplace.lock` = one `GrimoireLock` per plugin, `plugin` scope on the wire

| Option | R | P | S | U | Σ | Note |
|---|---|---|---|---|---|---|
| **a. In memory `MarketplaceLock { metadata, plugins: BTreeMap<String, GrimoireLock> }`; `plugin` exists only on the wire; each plugin resolved as its own `DesiredSet` through the unchanged resolver** | 3 | 3 | 3 | 3 | **30** | `LockedArtifact` and the `grimoire.lock` code path untouched; only lock (de)serialization learns the scope |
| b. Make every `(kind, name)` uniqueness site scope-aware (resolver, effective_set, prune, installer, lock, update — 8 files) | 2 | 2 | 1 | 3 | 20 | Touches install/prune paths export never uses; high regression surface |
| c. `LockedArtifact.plugin` in memory + `split_by_plugin`/`combine` (round-0 proposal) | 3 | 2 | 2 | 3 | 25 | Every constructor, sort and `content_equal` changes; `grimoire.lock` bytes identical only by test, not by construction |
| d. Parallel lock struct with its own entry type, or one lockfile per plugin | 1 | 3 | 2 | 2 | 19 | The "second lock path" the Reuse mandate forbids; no ecosystem uses N lockfiles |

**Chosen: a.** `MarketplaceLock` is **a map of `GrimoireLock`, not a parallel
lock struct**: its parts are the exact type `resolve_lock` returns and
`update` rolls forward; it adds a key, no entry type and no second IO path.
Sub-decisions:

- **Wire field**: `RawLockedArtifact.plugin: Option<String>` (`#[serde(default)]`,
  `#[schemars(skip)]`), emitted right after `name` by the marketplace
  serializer view only. `LockedArtifact` gains nothing. The published
  `grimoire-lock` schema is **byte-identical** and `grimoire.lock` bytes are
  identical **by construction** (its serializer is not touched).
- **IO**: `lock_io::load_marketplace` / `save_marketplace` share the raw
  parse, serializer, atomic write and `generated_at` logic with `load`/`save`.
  Entries sorted by `(plugin, name)`; `generated_at` preserved iff the plugin
  key sets are equal and every part is `content_equal` (existing fn,
  unchanged) to its previous part.
- **Flavor validation**: `lock_io::load` rejects any entry with `plugin` or a
  non-empty `[[plugin]]` table (78, like any malformed lock — surfaced by
  callers that propagate load errors; `update`/`lock`/`add` keep treating an
  unreadable lock as absent). `load_marketplace` requires `plugin` on every
  entry, an empty `[[bundle]]` (the bundle cache exists for `add`/`remove`
  effective-set mutation, which marketplaces do not have; provenance stays on
  each entry), and runs `SkillName::parse` on every entry `name` and `plugin`
  value — a hand-edited lock cannot inject `../` or an absolute name (78).
- **`declaration_hash`** (marketplace canonical form, own constant
  `MARKETPLACE_HASH_VERSION = 1`, independent of the grimoire
  `DECLARATION_HASH_VERSION`): SHA-256 over JCS of the sorted, deduped
  expanded include list — per plugin (stored in a `[[plugin]] { name,
  declaration_hash }` wire table, `#[schemars(skip)]`, omitted when empty) and
  for the whole manifest as `{"<plugin>": […]}` (`[metadata].declaration_hash`).
  Computed locally from `resolve_reference` expansion — no network, so
  staleness is decidable offline. **Excluded:** `description`, `version`,
  `rename` (not resolution inputs; editing them must not re-pin).
- **Staleness granularity** — options: per-plugin hashes (R3 P3 S2 U3, Σ 28),
  whole-file hash (R3 P3 S3 U1, Σ 26: editing plugin A rolls plugin B's
  floating refs, contradicting the ratified "a team zip changes only on
  purpose"), per-plugin inference from provenance (unsound: direct entries
  strip the declared tag). **Chosen: per-plugin.** `marketplace.lock` is a new
  file, so the `[[plugin]]` table costs no frozen-format change. Plugin P is
  stale iff absent from the lock or its hash differs; plugins no longer
  declared are dropped; export re-resolves only stale plugins and carries fresh
  ones byte-identically.
- **Write policy**: declared `grim export plugin` writes the lock when it
  re-resolved or dropped any plugin (or the lock was absent), **after** every
  output is placed. Advisory `ConfigFileLock` on the manifest path for the
  resolve+write window (75 on contention). Lock path = manifest path with
  extension `.lock` (`marketplace.toml` → `marketplace.lock`).

Reversibility: **one-way** for the wire field names, the `[[plugin]]` table
and the flavor rules (a frozen file format); two-way for staleness policy
details (additive).

### D3 — `grim update --marketplace <PATH> [<selector>…]`

| Option | R | P | S | U | Σ | Note |
|---|---|---|---|---|---|---|
| **a. Value-required flag on `update`; positional `names` reused as selectors** | 3 | 3 | 3 | 2 | **28** | No positional ambiguity; same verb, same resolver calls |
| b. Optional value defaulting to `./marketplace.toml` | 3 | 3 | 2 | 3 | 26 | `update --marketplace team` would parse `team` as the path |
| c. Separate verb (`grim export --update`) | 1 | 3 | 2 | 1 | 17 | Rejected by owner (Update entry point) |

**Chosen: a.** Selector grammar: `<plugin>` | `<plugin>:<member>` (member =
lock name, pre-rename, matching every kind with that name — as `grim update
<name>` does; split on the single `:`). Malformed → 64; unknown plugin or
member → 79 (parity with `grim update <unknown>` = `TagNotFound` → 79).
**Seam**: the first statement of `update::run` branches to `run_marketplace`
*before* `scope_resolution::resolve` / `InstallTarget::parse` — install scope
resolution, installer, prune, reap, `sync_config` and install state are
unreachable. Its registry context comes from `resolve_fetch_scope` anchored
at the manifest (D1). The roll-forward itself is **one function**:
`update::roll_forward`, extracted verbatim from `update::run`'s
full/partial/absent-previous branching and called by both `update::run` and
the marketplace resolution seam (C-009, C-034). No selectors → every plugin
rolled; `<plugin>` → that plugin rolled regardless of staleness, others
carried; `<plugin>:<member>` → partial roll of that plugin, which must be fresh
(else 65 `stale-lock`); a plugin-level selector subsumes member selectors of
the same plugin. `--force`/`--client` → 64 (clap `conflicts_with`);
`--global`/`--config` → 64 (global flags, runtime check).
**Report**: the existing `build_report`, keyed on `(plugin, kind, name)` and
called with no pruned/reaped rows, plus an additive always-present `plugin`
field (`null` on a normal update). Reversibility: one-way (CLI).

### D4 — `grim export plugin` CLI

```
grim export plugin [<REF>…] [--name <N>] [--plugin <P>…] [--marketplace <PATH>]
                   [--client <LIST>…] [--zip] [-o, --output <DIR>] [--version <V>] [--force]
```

- **Input modes** (mutually exclusive): ad-hoc `<REF>…` (+`--name`) · declared
  `--plugin <P>…` · neither ⇒ every declared plugin. `--plugin` with refs → 64
  (clap). Exactly one ref ⇒ name derived from the ref's binding name (a
  non-`SkillName` derivation → 64 with "pass --name"); 2+ refs without `--name`
  → 64; `--name` with `--plugin` or zero refs → 64. `--marketplace` with refs → 64.
- **Ad-hoc refs** are an in-memory one-plugin manifest through the same
  resolution seam as a declared one (same `DesiredSet` build, same resolver,
  no lock read or written), anchored at the cwd.
- **Member collisions** — the ratified rule "same name + different digest →
  error naming both sources", applied strictly, with no resolver change:
  direct-vs-direct includes compare expanded identifiers (same → deduplicated,
  different → **78**); after resolution, before the bundle snapshots are
  dropped, any two entries with the same `(kind, name)` (e.g. a path include
  and a bundle member, which the resolver keeps as duplicates) → 78, and a
  direct registry include whose `(kind, name)` matches a bundle member with a
  different expanded identifier → 78 naming both sources (identical →
  deduplicated). Bundle-vs-bundle keeps the resolver's `BundleConflict` (78).
  ⚑ Identifier comparison is stricter than the discussion's digest comparison
  (`x:1` and `x:1.2` at one digest → 78); loosening later is additive,
  tightening would not be. The `grimoire.toml` "direct wins" override does not
  apply inside a plugin.
- **Latest major**: phase 1 honours each ref's own tag (a user wrote `:0` or
  `:1` deliberately); "latest major only" is a phase-2 `export marketplace`
  concern where refs come from an index. Recorded, not built.
- **Client default**: `--client` → `[options].clients` of the ambient scope
  (project config if discovered, else global) → `agents`. No detection.
  Explicit client with no plugin family → **78** (same code as an unknown
  client name, `InstallErrorKind::UnsupportedClient`); config-derived one →
  skipped with a stderr note; if filtering empties the config list → `agents`.
- **Output**: `<DIR>/<plugin>.<client>/` (top level = plugin root) or
  `<DIR>/<plugin>.<client>.zip` (no wrapping folder). `-o` default `.`,
  created if absent. Every staged destination and zip entry name is checked to
  stay inside its root (no `..`, absolute, drive prefix or `\`) → 65.
- **Overwrite** — options: refuse unless `--force` (Σ 27), always replace
  (Σ 22: deletes a user directory that happens to share the name), never
  replace (Σ 21: CI reruns need `rm`). **Chosen: refuse** with 65 reason
  `untracked-destination` (existing forceable reason, reused verbatim).
  Placement is atomic no-replace: a zip via `hard_link` (EEXIST → 65), a
  directory via `rename` (fails on an existing non-empty directory → 65; a
  concurrently created *empty* directory can still be replaced — documented
  residual race). `--force` renames a zip over the old file; a directory's old
  path (a symlink is moved, never followed) is first renamed into the staging
  dir, then the new one renamed in. ⚑ `--force` is beyond the discussion's
  draft flag list.
- **Atomicity**: resolve → stage every (plugin, client) output under
  `<DIR>/.grim-export-*` (rename name checks, C-021, already ran before any
  fetch) → stale-reference scan → zip → collision check for every
  final path → place each → then write the lock. Any error before the first
  placement writes nothing (a freshly created `<DIR>` may remain empty).

Reversibility: one-way (CLI, frozen at 1.0).

### D5 — Rendering seam

| Option | R | P | S | U | Σ | Note |
|---|---|---|---|---|---|---|
| i′. Members via each client's `ClientTarget::materialize` into export-computed plugin paths; Claude-family `.mcp.json` via the client vendor's `mcp_entry` + `json_splice::upsert_member` from `""`; Agent Plugins MCP omitted in phase 1 | 3 | 3 | 3 | 1 | 25 | Only reuse, but Copilot/Codex/Cursor plugins lose every MCP server — half of the Agent Plugins 1.0 shape |
| **i. As i′ plus a family-level Agent Plugins `mcp.json` emitter (a small `McpDescriptor` projection in the style of `Vendor::mcp_entry`, C-036)** | 3 | 3 | 3 | 3 | **30** | `mcp.json` is one of the two components of the Agent Plugins shape the mandate already licenses ("the two plugin.json shapes"); mapping settled in `research_plugin_manifest_fields.md` |
| ii. Per-client native `mcp_entry` for both families | 2 | 2 | 2 | 1 | 17 | Codex's is TOML, Copilot's is its own schema — wrong file shape |
| iii. Omit MCP everywhere | 3 | 3 | 3 | 1 | 26 | Drops a working Claude path for no saving |

**Chosen: i** (amended 2026-09-27 by the meta-orchestrator; i′ was the original choice). Details:

- **Scope passed to `materialize`: `Global`** — plugin content is user-level
  and workspace-free; it only reaches `rule_index`, and rules are never emitted
  in phase 1.
- **Admission gate** per member and client: the family admits the kind
  (Claude: skill, agent, mcp; Agent Plugins: skill, mcp) **and**, except for
  Agent Plugins MCP, `client.vendor().kind_support(kind) != Declined` —
  `mcp.json` is the family's file, not the client's, so a client whose
  install path declines MCP (`agents`) still carries it. `kind_surface` is *not*
  consulted — it answers "does the native directory exist at this scope",
  which a plugin tree does not use (this is why OpenClaw skills export).
  Otherwise the member is omitted with a reason: `no-format-surface`,
  `client-declined`, `not-representable` (`mcp_entry` → `None` or a pointer
  outside `/mcpServers/`, or C-036 declining ws / oauth / an unexpandable
  `${…}`). Agent Plugins `mcp.json`: `http` → `streamable-http`, `sse` and
  `stdio` copied, refinement fields (`timeout`, `always_load`,
  `headers_helper`) dropped as every vendor projection does. Plugin
  placeholders are renamed per family and other `${VAR}` references warn
  rather than decline (see Amendment 2026-09-28).
- **Paths** (export-computed, never `path_for`): `skills/<name>/`,
  `agents/<name>.md`, `.mcp.json` (Claude family); `skills/<name>/`,
  `mcp.json` (Agent Plugins).
- **Staging** reuses the installer's fetch→verify→unpack→`locate_canonical`
  block, extracted as one `pub(crate)` function used by `install_one` and
  export (C-017) — not a copy. MCP members go `fetch_verified_layer` (made
  `pub(crate)`) → `McpDescriptor::from_layer_bytes`, as `install_mcp` does.
- **Never** emitted/run: `bin/`, rules, `sync_config`, install state, anchors,
  vendor detection, `shared_skills` pool options.

Reversibility: two-way (render layout is outside the 1.0 contract,
`adr_render_layout_stability.md` §1).

### D6 — Plugin manifests and version

Claude (`.claude-plugin/plugin.json`) and Agent Plugins (`plugin.json`),
serialized from fixed-order `#[derive(Serialize)]` structs, 2-space pretty,
one trailing `\n`:

```json
{ "name": "team", "version": "1.4.0+3f9a0c12b7de", "description": "…" }
{ "$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
  "name": "team", "version": "1.4.0+3f9a0c12b7de", "description": "…" }
```

No other keys (Agent Plugins is `additionalProperties:false`; `homepage`,
`author`, `keywords` are phase-2 additive from marketplace-level metadata ⚑).

- **Version base**: `--version` (leading `v` stripped) → `[plugins.x].version`
  (declared) / the single ref's `org.opencontainers.image.version` annotation
  (ad-hoc, exactly one ref; read from the **pinned** manifest the resolution
  captured — the bundle pin or the single entry's pin — never by re-resolving
  the floating tag) → `0.0.0`. Must parse as semver with empty build metadata
  (pre-release allowed) → else 65 (publish `--version` precedent).
- **Suffix**: `+` + first **12** hex of SHA-256 over the UTF-8 lines
  `<kind>\t<emitted name>\t<content digest>\n`, sorted, one per plugin member
  (all members, client-independent). Content digest = manifest digest
  (registry) or pack hash (path). ⚑ Uses the *emitted* (post-rename) name, so a
  rename change is a new version. 12 not 7: 48-bit birthday bound ≈ 16.7M vs
  ≈16K at marketplace scale (research). Options weighed: 7 hex (Σ 24),
  12 hex (Σ 28), full 64 (Σ 22: unreadable in UIs).
- **Accepted risk**: the hash covers member digests and emitted names only. A
  declared `description` edit or a grim renderer change yields new bytes under
  the same version, which string-comparing harnesses will not offer as an
  update. Mitigation: bump `version`; stated in the docs (C-032).
- **Description**: the base alone, at most 500 UTF-16 units; the fixed on-ramp
  sentence `Packaged by grim (https://grimoire.rs); install grim for pinned, updatable installs.`
  when there is no base. Base = `--description` / `[plugins.x].description` /
  the single ref's `org.opencontainers.image.description` (same pinned source).
  An authored base over 500 exits 65; an annotation is cut (last whole sentence
  keeping half the budget, else word boundary + `…`). A generated root
  `README.md` carries the name, the uncut base, `Omitted for <client>: <kind>
  <name>, ….` and the on-ramp. Amended 2026-09-27 on owner review: the
  space-joined description (base, omission sentence, on-ramp) read badly once
  cut to fit.

Reversibility: one-way for the version grammar and hash input (a change
fires a phantom update in every harness); two-way for description wording.

### D7 — Rename (`[plugins.x.rename] strip_prefix`)

Applied after resolve, before render, per plugin. A member whose lock name
starts with the prefix is emitted as the remainder; others keep their name.
Every emitted name, renamed or not, must pass `SkillName::parse`.
**Fails closed (65, nothing written)** when:

1. the remainder is empty or not a valid `SkillName` (`hex` from `hex-plan` → `-plan`);
2. two members of one kind share an emitted name;
3. **stale reference**: any file under `skills/` or `agents/` of any client's
   staged tree (non-UTF-8 files skipped) contains an old name of a renamed
   member as a token — an occurrence not preceded and not followed by
   `[A-Za-z0-9_-]`. Catches `../hex-core/`, `/hex-plan`, `skills/hex-plan/`,
   prose `hex-plan`. Reported as `<client>: <relpath>:<line>: '<old>'`, all
   hits, capped at 50 lines + count.

Skills rename through the existing `render::rebind_skill_name` hook (binding =
emitted name); agents through a sibling `render::rebind_agent_name` applied by
export only; MCP by the `mcpServers` key. No reference rewriting. Options for
the scan: bare token (Σ 27, chosen — one rule, prefixed names are distinctive),
path/slash patterns only (Σ 22, misses prose), no content scan (Σ 18, violates
ratified fail-closed). Reversibility: two-way (loosening a check is additive).

### D8 — Zip

`zip = { version = "8.6", default-features = false }` (4 permissive transitive
crates; `deny.toml`-clean). Every entry: `CompressionMethod::Stored`,
`DateTime::default()` (1980-01-01), `unix_permissions(0o644)`, no extra fields,
no directory entries, paths `/`-joined relative to the plugin root, sorted by
byte order; dotfiles included. Entry names with `\`, a drive prefix, a `..`
component or non-UTF-8 bytes are refused. Tree output contract = set of
relative paths + file bytes (modes are not part of it; published layers carry
none). Options: `zip` crate (Σ 28), hand-rolled Stored writer (Σ 19, risky
format code on a security-adjacent artifact), `async_zip` (Σ 18, pre-1.0).
Reversibility: two-way (implementation detail behind a byte-reproducibility
test).

### D9 — Exit codes (no new code, no new `ErrorReason`)

| Failure | Exit | `reason` |
|---|---|---|
| Flag grammar: `--plugin` + refs, 2+ refs w/o `--name`, `--name` w/o exactly-ad-hoc, `--marketplace` + refs, malformed update selector, `--marketplace` + `--force`/`--client`/`--global`/`--config` on update, invalid `--name` | 64 | — |
| `marketplace.toml` missing/oversized/unreadable/parse/unknown key/invalid name/empty `include`/bad `version`/not `*.toml`/named `grimoire.toml`; bad `--version`; no plugin declared when selecting all | 65 | — |
| Rename: empty/invalid remainder, collision, stale reference | 65 | — |
| Every member omitted for a (plugin, client) — empty plugin | 65 | — |
| Staged destination or zip entry escapes its root | 65 | — |
| Output path exists without `--force` | 65 | `untracked-destination` |
| `<plugin>:<member>` selector on a stale plugin | 65 | `stale-lock` |
| Malformed `marketplace.lock` (incl. invalid name) / `grimoire.lock` scope violation | 78 | — |
| Unknown client or client with no plugin family (explicit) | 78 | — |
| Member conflict between includes / bundle conflict | 78 | — |
| Unknown `--plugin` / update selector / member | 79 | — |
| Tag or bundle not found | 79 | — |
| Registry unreachable / auth / offline miss | 69 / 80 / 81 | — |
| Manifest lock contended | 75 | `locked` |
| Filesystem failure | 74 (77 on EPERM) | — |

New typed error `crate::export::ExportError` joins `crate::error::Error` and
is mapped exhaustively in `classify` (no string matching). The marketplace
resolution seam returns `crate::error::Error`, so resolver, lock and access
errors keep their existing classification. Reversibility: one-way per row.

### D10 — JSON output

`grim export plugin --format json` → `{"items":[…]}`, one item per
(plugin, client), ordered by plugin then client in `--client` order:

```json
{"plugin":"team","client":"claude","family":"claude","format":"zip",
 "path":"/abs/dist/team.claude.zip","version":"1.4.0+3f9a0c12b7de",
 "members":[{"kind":"skill","name":"plan","lock_name":"team-plan","pinned":"ghcr.io/acme/team-plan@sha256:…"}],
 "omitted":[{"kind":"rule","name":"style","reason":"no-format-surface"}]}
```

Literals: `family` ∈ {`claude`, `agent-plugins`}; `format` ∈ {`dir`, `zip`};
`reason` ∈ {`no-format-surface`, `client-declined`, `not-representable`}. Every field always present (arrays `[]`). Plain: table
`Plugin | Client | Version | Path | Omitted` (count). Update: see D3.
Reversibility: one-way (additive-only thereafter).

### D11 — Principle 9 / constitution gate

| Frozen surface touched | Change | Additive proof |
|---|---|---|
| CLI | new `export` group + `plugin`; `update --marketplace` | New names only; no existing flag/arg changes meaning |
| `grim update --format json` | `plugin` on every row | Always-present-null field, the documented additive rule |
| `grimoire.lock` format | `RawLockedArtifact.plugin` and `RawLock` `[[plugin]]`, both `#[schemars(skip)]`, rejected by `lock_io::load` | Accepted-input set and written bytes unchanged; the `grimoire.lock` serializer and `LockedArtifact` are untouched |
| `grimoire-lock` schema URL | none | Schema bytes identical (test C-031) |
| Exit codes / error doc | none new | Reuses 64/65/74/75/78/79/80/81 and reasons `untracked-destination`, `stale-lock`, `locked` |
| `--client` semantics | reused name set | Install/update resolution untouched; export adds its own default chain |
| Published schema URLs | none added | `test_check_urls.py` untouched |
| New frozen contracts | `marketplace.toml`, `marketplace.lock` flavor + `[[plugin]]` table, export CLI + JSON, omission reasons, version grammar + hash input | New surfaces; frozen from 1.0 |

Unstable (outside the contract, like vendor layout): the bytes of rendered
member files inside an export (they follow the vendor renderers).

**Constitution Deviations:** none.

### D12 — Relation to `adr_render_layout_stability.md`

That ADR reserves an *install* render mode (`[options] render = "plugin"`):
grim-owned plugin sources registered into a harness via `sync_config`,
recorded in install state. Export is not install: it writes to a user-chosen
`-o`, registers nothing, records nothing, and adds no config key. The
reservation stands unchanged; a future install mode can reuse export's
manifest emitters and plugin layout as its source-rendering half. The
"plugins as a new `ClientTarget`" rejection (its Option 2) is honoured:
export maps existing clients to families, adding no client.

### D13 — Recorded supersessions and deferrals

- The discussion's "two clients sharing a format get byte-identical files"
  (Output naming) is superseded by per-client rendering (Phase-1
  settlements › Rendered like install): two outputs are identical only when
  the two vendors render identically.
- Agent Plugins MCP ships in phase 1 (D5 option i; the earlier omission was
  overturned by the meta-orchestrator under goal I4, no feature cutting).
- `--locked` for export (fail instead of refreshing a stale lock) is a
  follow-up, not phase 1; adding it later is additive.

## Decision Outcome

**Chosen:** D1a, D2a (per-plugin staleness), D3a, D4 as specified, D5 i,
D6, D7 bare-token scan, D8 `zip` Stored, D9–D10 tables, D11 no deviations,
D12 no conflict, D13 as recorded.

### Migration / rollout

None required: new files (`marketplace.toml`/`.lock`) and a new subcommand.
Existing `grimoire.lock` files parse and re-serialize byte-identically; the
published schema does not change; `grim update` without `--marketplace` is
unchanged except the additive `plugin: null` field.

### Consequences

**Positive:** one resolve/lock/update/render path serves install and export;
editing one plugin never moves another's pins; reproducible bytes from day
one; the Claude-app zip upload works with no grim on the consumer.

**Negative:** `marketplace.lock` carries a marketplace-only `[[plugin]]`
table; Agent Plugins MCP drops ws / oauth / unexpandable-reference servers
(named), and exports other `${VAR}` references with a warning though a
client may pass them literally (Amendment 2026-09-28); identifier-strict
member conflicts reject some digest-equal duplicates; strict content scan
stops today's `hex` bundle until its cross-links are removed (intended).

**Risks:**
- Copilot/Codex/Cursor version comparison is undocumented; a semver-aware
  consumer would ignore `+<hash>`. Mitigation: documented as load-bearing; a
  real bump still moves the base version.
- Same version, new bytes (D6 accepted risk). Mitigation: bump `version`.
- Claude-app zip reader tolerance (Stored, no dir entries) unverified.
  Mitigation: the most conservative zip subset; acceptance round-trips with
  Python `zipfile`.
- Plugin-embedded skill folder-name enforcement in the Claude app is
  unconfirmed. Mitigation: rename keeps folder = frontmatter name by
  construction.

## Technical Details

### Architecture

```
marketplace.toml ─┐                     ┌─ per selected plugin: plugin_set ─ update::roll_forward (resolver unchanged) ─ conflict check ┐
<ref>… --name ────┴─ export::resolve ───┤                                                                                             ├─ MarketplaceLock ─ lock_io::save_marketplace
                                        └─ declaration hashes (local, per plugin + whole)                                             │
grim update --marketplace ── run_marketplace ── same seam ── build_report ────────────────────────────────────────────────────────────┘
grim export plugin ── export::stage ── per plugin part ─ rename ─ stage_locked_artifact (installer seam)
                      └─ per client: family gate ─ ClientTarget::materialize / mcp_entry+json_splice
                         ─ stale scan ─ plugin.json ─ [zip] ─ containment check ─ place outputs ─ write lock
```

### Reuse map (new behaviour → seam, or justified new code)

| Behaviour | Seam / new code |
|---|---|
| Ref → kind + binding name + source | extracted `add` declare seam `declare_reference` (kind inference, path shape, intrinsic path name, `SkillName` guard) |
| Registry context | `command::resolve_fetch_scope` → `fetch::FetchScope` (anchored at the manifest) + `access_seam` |
| Resolve / partial resolve | `resolver::resolve_lock`, `resolve_lock_partial` (unchanged) |
| Lock type, IO, advisory lock | `GrimoireLock` (one per plugin, keyed in `MarketplaceLock`), `lock_io` raw parse / serializer / atomic write / `content_equal`, `ConfigFileLock` |
| Update roll-forward | extracted `update::roll_forward` (C-034), called by `update::run` and `resolve_marketplace` |
| Update report | `update::build_report` keyed on `(plugin, kind, name)` |
| Fetch + verify + unpack | extracted `installer::stage_locked_artifact`; `fetch_verified_layer` `pub(crate)` for MCP |
| Member render | `ClientTarget::materialize`, `render::rebind_skill_name`; **new** `rebind_agent_name` (sibling, ~20 lines) |
| Claude-family MCP file | `Vendor::mcp_entry` + `json_splice::upsert_member` |
| **New:** Agent Plugins `mcp.json` entry (C-036) | mandate item (part of the Agent Plugins shape); assembled by the same `upsert_member` routine |
| Client list parsing | extracted `target::parse_client_list` (from `InstallTarget::parse`) |
| Manifest version check | `semver` crate (already a dependency) |
| **New:** `marketplace.toml` parse + hashes | mandate item |
| **New:** two `plugin.json` emitters, family map, description | mandate item |
| **New:** rename + stale scan | mandate item |
| **New:** zip writer + containment checks | mandate item |
| **New:** `ExportReport`, `ExportError`, CLI wiring | required glue |

### Module placement

- `src/command/export.rs` — clap group, `run_plugin`
- `src/export.rs` — module root: `mod` lines and re-exports only
- `src/export/marketplace.rs` — `marketplace.toml` load, declaration hashes
- `src/export/resolve.rs` — `plugin_set`, `resolve_marketplace`, `PluginSelection`, `MarketplaceResolution`, lock path
- `src/export/stage.rs` — export orchestrator: per plugin × client staging, placement, lock write
- `src/export/family.rs` — client→family, admission gate, `plugin.json` emitters, description
- `src/export/rename.rs` — strip_prefix + stale scan
- `src/export/archive.rs` — zip writer
- `src/export/export_error.rs`
- `src/api/export_report.rs`
- `src/lock` — `MarketplaceLock`, `lock_io::{load_marketplace, save_marketplace}`
- touched: `src/lock/{grimoire_lock,locked_artifact,lock_io,lock_error}.rs`,
  `src/config/hash.rs` (`MARKETPLACE_HASH_VERSION`),
  `src/command/update.rs`, `src/api/update_report.rs`, `src/install/{installer,render,target}.rs`,
  `src/command/add.rs`, `src/main.rs`, `src/app.rs`, `src/error.rs`, `Cargo.toml`

## Validation

- [ ] Acceptance: every scenario S-001…S-030 in the spec
- [ ] Byte-reproducibility test (tree hash + zip sha256 across two runs, `TZ` varied)
- [ ] `grim schema --kind lock` output byte-identical to `main`
- [ ] Security review of `--force` placement, zip writer, containment checks, staged-tree scan

## Amendment 2026-09-27 — share a project as a plugin

Owner review reversed the "0-ref lock input dropped" note in
`.agents/discussions/harness-native-marketplaces.md`, as a thin wrapper
rather than a second lock system (plan: `.agents/plans/plan_export_project.md`):

- **`grim export plugin --project`** renders the resolved scope's fresh
  `grimoire.lock` (`install::fresh_lock`: missing 79, stale 65) as one plugin.
  No resolution, no lock written. Path sources anchor at the project dir; a
  drifted one exits 65 pointing at `grim lock` there. Name: `--name`, else
  `[plugin].name`, else 64 (no directory default, as before).
- **`[plugin]` in `grimoire.toml`** (`config::plugin_meta::PluginMeta`:
  `name`, `description`, `version`, `logo`, `rename`) — optional, outside the
  declaration hash (existing locks stay fresh), invalid at load → 78. The four
  scalars are `grim config` keys (`plugin.*`, project scope, bad value 65).
  The name/version/description grammars moved from `export::marketplace` to
  `config::plugin_meta` so config does not depend on export.
- **`project = "<dir>"` in `marketplace.toml`**, exclusive with `include`
  (both/neither 65). Pins come from that project's lock, never from
  `marketplace.lock` (no part, outside the declaration hashes, skipped by
  `grim update --marketplace`; naming one there → 64). Metadata falls back
  per field: flag > `[plugins.x]` > project `[plugin]` > none.

## Amendment 2026-09-28 — plugin placeholders and env references in MCP

Review round 1 of the export branch (R1-19). Contracts: design record
C-020 and C-036, amendments of the same date.

- **§9.2 fact.** The Agent Plugins spec expands only `${PLUGIN_ROOT}` and
  `${PLUGIN_DATA}`, in `args`, `env` values and `cwd`. It expands no
  environment variables.
- **Rename to the spec.** An Agent Plugins export (`codex`, `cursor`,
  `copilot`, `agents`) renames `${CLAUDE_PLUGIN_ROOT}` / `${CLAUDE_PLUGIN_DATA}`
  to `${PLUGIN_ROOT}` / `${PLUGIN_DATA}`.
- **Rename to Claude.** A `claude` export does the reverse, in every field
  including `url` and headers, where Claude expands its placeholders.
  Export-only: `grim install` writes the descriptor as is. This is the one
  install/export divergence in MCP rendering. Droid, Junie and OpenClaw
  keep the descriptor's spelling.
- **Warn, don't decline.** In an Agent Plugins export, any other `${VAR}`
  in `args`, `env` values or `cwd` is exported with a warning, so the
  server works once its client expands environment variables. A `${…}` in
  `command`, `url`, an env key or a header still omits the server
  `not-representable`. Reversibility: one-way once released. Tightening to
  decline later removes output, a breaking change under Principle 9.
  Chosen because a server that works for its placeholder parts, and works
  fully once its client adds expansion, beats omitting it.

## Links

- `.agents/adr/adr_render_layout_stability.md` (D12)
- `.agents/adr/adr_grim_publish.md` (manifest precedent)
- `.agents/adr/adr_local_path_sources.md` (path includes)
- `.agents/adr/adr_effective_set_mutations.md` (bundle cache, not carried)
- `.agents/adr/adr_mcp_percall_scope_fetch_render.md` (render ≠ install precedent)

---

## Changelog

| Date | Author | Change |
|------|--------|--------|
| 2026-09-27 | Architect (hex-plan xhigh) | Initial proposal |
| 2026-09-27 | hex-plan review round 1 | Amendments A1–A20: `MarketplaceLock` map with wire-only `plugin`; per-plugin staleness; strict member conflicts; path containment; error channels; pinned version source; selection model; manifest-anchored registry context; `declare_reference`/`roll_forward` seams; atomic placement; D13 supersessions. Status → Accepted |
| 2026-09-27 | hex-plan re-validation | R1–R9: `grim context` keeps exit 0 with `lock_error`; MCP lock/emitted names use a containment check, not `SkillName`; part metadata copies top-level `[metadata]`; `DeclareError` → exit mapping with explicit offline check; `.toml`-stem and absolute manifest path; D4 atomicity order |
| 2026-09-27 | meta-orchestrator decision | D5 → option i: Agent Plugins `mcp.json` emitter (C-036, S-031); `not-yet-supported` reason removed; Agent Plugins MCP admitted per family, not gated on client `kind_support` |
| 2026-09-27 | owner review | Amendment: `--project`, `[plugin]` in `grimoire.toml`, marketplace `project` plugins |
| 2026-09-28 | review round 1 (R1-19) | Amendment: MCP plugin placeholders renamed per family (claude export reverses the Agent Plugins rename, export-only); other `${VAR}` in args/env values/cwd warn instead of decline; D5 bullet and Negative consequence point to it |
