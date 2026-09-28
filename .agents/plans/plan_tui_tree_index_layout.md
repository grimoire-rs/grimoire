# Plan: TUI tree follows the index layout for index-backed sources

## Status

- **Plan:** plan_tui_tree_index_layout
- **Active phase:** 5 — Finalized for landing (review rounds 2/2 complete)
- **Step:** round 1 xhigh Needs Work → fixed; round 2 xhigh Needs Work (no Block/High) → fixed; task verify + docs:check green; cross-model pass skipped both rounds (codex quota)
- **Last update:** 2026-09-28

## Classification

| Axis | Value |
|---|---|
| Type | Feature (UX simplification) |
| Scope | Small–Medium: 1 data field threaded catalog → TUI, 1 tree segmentation branch |
| Reversibility | Two-way door: tree is a pure projection (`adr_projection_over_index.md`), cache field is additive |
| Subsystems | `src/catalog` (index_source, registry_catalog), `src/tui` (state, tree) |
| CLI surface change | None: no flag, no JSON field, no exit code |

## Objective

Users find the tree view too deep. Under an index-backed source root the
tree currently groups by the **physical OCI reference**
(`ghcr.io/org/foo-skills/x` → 3–4 group levels), because
`tree::display_split` falls back to the full `registry/repository` when the
index locator is not an OCI prefix. The index already has its own, flatter
layout: `index/<host>/<namespace>/<name>`. Group index rows by that layout
instead.

Before (index source root, OCI path):

```
▾ grimoire (https://index.grimoire.rs)
  ▾ ghcr.io/grimoire-rs
    ▾ skills
        grim-usage
```

After (index layout; single-child chains path-compress as today):

```
▾ grimoire (https://index.grimoire.rs)
  ▾ github.com/grimoire-rs
      grim-usage
```

The OCI reference stays visible in the detail pane and in the flat list's
Repo column: the tree stops using it as structure, it does not hide it.

## Scope

### In Scope

- `CatalogEntry.index_path: Option<String>`: `<host>/<namespace…>/<name>`
  as the index placed the pointer.
  - HTTP transport: `all.json` element's derived `namespace` field + `name`.
  - Git transport: the `metadata.json` directory path relative to `index/`.
- Thread to `TuiRow.index_path`.
- `tree::build`: for rows with a valid `index_path`, the path under the
  source root is the index path's segments; the leaf label is the index
  `name`.
- Label hygiene on the index-supplied segments (index content is untrusted).

### Out of Scope

- Flat list: unchanged (Repo column keeps the OCI ref: that is where the
  "where do bytes come from" legibility lives).
- OCI `_catalog` sources: unchanged; they have no index layout.
- `grim search --format json` / MCP output: no new field (YAGNI; nothing
  consumes it outside the TUI).
- Removing `tree_separators` / `group_by_type`: the config knobs are a
  released surface (Principle 9). Separators simply do not apply to index
  segments; see Key Decisions.

### Scope additions (2026-09-28)

Owner-requested follow-ups landed on this branch:

- TUI search keeps the flat view until the query is cleared
  (`fix(tui): keep the flat view until the search query is cleared`).
- Two-stage Esc: the first clears the query, the second leaves search
  (`fix(tui): make esc clear the search query before leaving search`).
- Export renames plugin placeholders per client and warns about env
  references Agent Plugins clients may not expand
  (`feat(export): translate plugin placeholders per client`). Recorded in
  `adr_harness_plugin_export.md`, Amendment 2026-09-28.
- The git walk takes placement from the directory. It skips symlinks, and
  gives no placement when `name` differs from the directory name (review
  rounds 1 and 2).

## Technical Approach

### Key Decisions

1. **Index layout wins only when present and valid.** Missing `namespace`
   (a third-party static index built before the field), or a segment failing
   validation → fall back to today's OCI-path split for that row. No error,
   `debug` log.
2. **Separators do not split index segments.** The index already defines
   the hierarchy; splitting `grim-usage` on `-` would reintroduce the depth
   we are removing. `tree_separators` keeps applying to OCI paths.
3. **`group_by_type` still inserts the type level** under the root: it is
   orthogonal to the path source.
4. **Leaf identity stays `rows` index + `repo`.** Selection anchors,
   marks, collapse keys for leaves are untouched. Group collapse keys change
   for index roots (new path) — acceptable: `collapsed` is ephemeral session
   state, never persisted.
5. **Validation of index segments:** non-empty, no `.`/`..`, no control or
   bidi characters (reuse the existing untrusted-label sanitizer from Phase 2
   bundle members), `/` only as separator. Reject → fallback (Decision 1).
   *Amended 2026-09-28:* shipped as an allowlist, not the sanitizer. Each
   segment is `[A-Za-z0-9._:-]`, 1 to 128 characters, never `.` or `..`, and
   a path has at most 16 segments (`index_source::index_path`).

### Architecture Changes

None structural. `display_split` gains an index-row branch; `build` stays a
pure projection over `rows[filtered]`.

## Backwards Compatibility

- **Catalog cache:** `index_path` is `#[serde(default, skip_serializing_if =
  "Option::is_none")]`. Old cache → field absent → OCI fallback until the
  next refresh (≤ TTL). Newer cache read by older grim → rejected by
  `deny_unknown_fields` and rebuilt: the established S-015 path, same as
  `RatingSummary.provider`. Test both directions.
- **Index wire format:** read-only use of already-specified fields
  (`namespace` in `all.json`, directory layout for git). No spec change.
- **Config:** no new keys; existing `[options.tui]` semantics unchanged for
  OCI sources.
- **Renderer / install layout:** untouched.

## JSON Interface

No change. `CatalogEntry` is not serialized into any command output
(verified: no `CatalogEntry` use under `src/command*`); only the cache holds
the new field.

## Exit Codes

No change. A bad `index_path` degrades to fallback, never an error.

## Implementation Steps (TDD)

1. **Tests first (catalog):**
   - `into_entry` maps `namespace` + `name` → `index_path`
     (`github.com/grimoire-rs/grim-usage`).
   - Missing `namespace` → `None`.
   - `walk_metadata` derives `index_path` from
     `index/<host>/<ns>/<name>/metadata.json`; nested GitLab group namespace
     (`gitlab.com/a/b/<name>`) keeps all segments.
   - Cache round-trip: old JSON without the field parses; new JSON with it
     is rejected by a copy of the pre-change struct (rebuild path).
2. **Tests first (tree):**
   - Index row groups by index path; leaf label = `name`.
   - Single-child chain compresses to `github.com/grimoire-rs`.
   - `tree_separators = ["-"]` does not split index segments; still splits
     an OCI row in the same tree.
   - `group_by_type` inserts type level above index segments.
   - Invalid segment (`..`, control char, empty) → OCI fallback.
   - Row without `index_path` → byte-identical to current output
     (regression guard on existing fixtures).
3. **Implement:** add field in `IndexPackage` (`namespace: Option<String>`),
   `CatalogEntry`, `TuiRow`; git walk passes relative dir; tree branch in
   `display_split` / `segments`.
4. **Acceptance (pytest):** one TUI-free check is not possible (tree is
   TUI-only); rely on unit tests in `tree.rs` + a catalog cache acceptance
   test that an index-backed browse persists `index_path` in the cache file.
5. **Docs:** `docs/` TUI page: one sentence that index-backed sources group
   by the index's namespace. Catalog drift review (`grim-usage` skill
   mentions the tree?) per `catalog/README.md`.
6. `task verify`.

## Files to Modify

| File | Change |
|---|---|
| `src/catalog/index_source.rs` | read `namespace`; derive `index_path` (HTTP + git walk) |
| `src/catalog/registry_catalog.rs` | `CatalogEntry.index_path` (optional, cache-additive) |
| `src/tui/state.rs` | `TuiRow.index_path` |
| `src/tui/tree.rs` | index-path branch in `display_split`/`segments`, validation |
| `docs/src/content/docs/…tui…` | one-line behavior note |
| `test/…` | cache persistence acceptance test |

## Risks

| Risk | Mitigation |
|---|---|
| Malicious index crafts deceptive group labels | Segment allowlist and caps (Decision 5); the root is still the configured source, so a label can never impersonate another registry root |
| Two entries with same `<ns>/<name>` but different kinds | Index spec: a name is claimed by one kind per namespace; if violated, both leaves render under the same group (distinct `rows` indices) — no collision in identity |
| Users relying on OCI-path grouping under index roots | Two-way door; if asked for, add `[options.tui] index_layout = false` later (not now) |

## Rollback

Revert the commit. Cache field is additive; older binaries rebuild.
