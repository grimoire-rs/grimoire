# ADR: Local Path Sources (path dependencies + dev-install)

## Metadata

**Status:** Accepted
**Date:** 2026-07-11
**Deciders:** Michael Herwig + Claude (planning session; handover `handover_local_path_deps.md`)
**Beads Issue:** N/A
**Related PRD:** N/A — plan artifact at `~/.claude/plans/handover-local-path-cuddly-treasure.md` (session-local)
**Tech Strategy Alignment:**
- [x] Decision follows Golden Path in `.claude/rules/product-tech-strategy.md` (no new crates; std + existing `tar`/`sha2` machinery)
**Domain Tags:** data | api
**Supersedes:** N/A
**Superseded By:** N/A

## Context

Every grim artifact today is sourced from an OCI registry: `grimoire.toml`
declares `name = "registry/repo:tag"`, the lock pins a registry **manifest
digest** (`PinnedIdentifier`, digest required by construction), and install
fetches the layer blob from the registry. There is no path from a local
directory into the vendor render engine — testing or maintaining a skill
requires a registry round-trip.

Driving use case: maintain one canonical skill/rule/agent/bundle in a repo
and render it into every co-worker's AI client (`claude`, `opencode`,
`copilot`) via clone + `grim install`. The expensive part — the per-vendor
render engine — already exists at install time (`src/install/vendor*.rs`,
seam `installer.rs` blob → `materializer.materialize`). What is missing is
a **local source** feeding that seam.

Two capabilities are wanted:

- **Declared path dependencies** — a `grimoire.toml` value may be a local
  path; `lock`/`update`/`status` detect directory changes and re-materialize.
- **Ad-hoc dev-install** — `grim install ./path` renders a local artifact
  once, without touching config or lock (throwaway test loop), yet remains
  visible to `status`/`update`/`uninstall`.

## Decision Drivers

- Reuse the existing render engine and lock/status/update flows unchanged.
- Deterministic, reproducible pins — same guarantees as registry digests.
- Full offline operation for local sources (`GRIM_OFFLINE`).
- Registry-only configs and locks must stay **byte-identical** (compat).
- Smallest wire-format change consistent with existing serde patterns.

## Industry Context & Research

**Research artifact:** `handover_local_path_deps.md` + 4 codebase exploration
passes (2026-07-10/11).
**Trending approaches:** Cargo `path = "../foo"` deps (re-checked at command
time, no watcher; absolute paths allowed but unpublishable), npm `file:`
deps, uv/pip editable installs.
**Key insight:** grim's canonical packing is already fully deterministic
(sorted entries, mode 0644, mtime/uid/gid zeroed, uncompressed tar;
unit-tested byte-for-byte contract) — a SHA-256 over the packed layer is a
sound digest substitute, so every digest-keyed flow works source-switched.

## Considered Options

### Option 1: Source-discriminant enums (chosen)

**Description:** Two internal enums — `DeclaredSource { Registry(Identifier),
Path(PathSource) }` (config layer) and `LockedSource {
Registry(PinnedIdentifier), Path { path, hash } }` (lock + install state).
Wire format via the existing `#[serde(try_from = "Raw*")]` XOR pattern
(`pinned` XOR `path`+`hash`), lock stays `lock_version = 1`.

| Pros | Cons |
|------|------|
| Exhaustive matches force every consumer to handle path sources | ~15 mechanical consumer sites re-typed |
| Registry-only wire bytes unchanged (compat proven by byte-identity tests) | Old grim exits 78 on a path-bearing lock |
| Mirrors existing `bundle`/`bundle_tag` XOR precedent | |

### Option 2: Path-capable `Identifier`

**Description:** Teach `Identifier`/`PinnedIdentifier` to carry a path form
(e.g. `path:./skills/x@sha256:…`).

| Pros | Cons |
|------|------|
| No consumer re-type | Pollutes the OCI grammar; every registry code path must reject path forms at runtime |
| | `PinnedIdentifier`'s "registry digest required" invariant becomes a lie |

### Option 3: Parallel path maps / serde-tagged enums on the wire

**Description:** Separate `[path-skills]` config tables or an internally
tagged lock entry.

| Pros | Cons |
|------|------|
| No XOR validation needed | New consumers silently skip path entries (no exhaustiveness) |
| | Codebase deliberately avoids serde `tag`/`untagged`; breaks single-namespace binding names |

## Decision Outcome

**Chosen Option:** Option 1 — source-discriminant enums with `try_from = Raw*`
XOR wire shapes.

**Rationale:** Exhaustiveness at compile time, zero wire change for
registry-only files, and it is the established pattern in this codebase
(`RawLockedArtifact`). Option 2 corrupts the OCI domain model; Option 3
trades compile-time safety for parser convenience.

### Recorded sub-decisions

1. **Pin = SHA-256 of the canonical packed layer** — tar for skill/rule/agent
   (`pack_skill_dir`/`pack_rule_file`/`pack_agent_file`), canonical JSON for
   bundles (`BundleManifest::to_layer_bytes`). Never touches `OciAccess`;
   fully offline for tar kinds.
2. **Config syntax: bare string values** — `my-skill = "./skills/my-skill"`.
   Discriminant: value starts `./`, `../`, or is absolute. No inline tables
   (`RawConfig` stays `BTreeMap<String, String>`).
3. **Path policy: absolute + relative allowed everywhere; absolute in
   PROJECT scope warns** (committed config is not portable); global scope
   silent. Relative anchor = the config file's directory (project:
   `grimoire.toml` dir; global: `$GRIM_HOME`). `grim add` rewrites to
   relative when the source lies inside the project.
4. **Lock stays V1, install state stays V2** — path entries add optional
   XOR-validated fields. Old grim fails with exit 78 (`deny_unknown_fields`)
   only on locks that actually contain path entries.
5. **Kinds v1: skill (dir), rule (file + support dir), agent (file), and
   local bundles with REGISTRY members.** Relative member refs in a local
   bundle are rejected (no registry identity to resolve against). Path
   members and `[mcp]` path values → error 65, tracked as follow-ups.
   **Local-bundle lock wire (recorded 2026-07-11):** a local bundle has no
   registry `repo`/`tag`/`pinned`, so `LockedBundle` becomes a source
   discriminant — `Registry { repo, tag, pinned }` XOR `Path { path, hash }`
   — via a `RawLockedBundle` + `TryFrom` XOR validator that mirrors
   `LockedSource` / `RawLockedArtifact` (sub-decision 1's pattern). The
   registry arm serializes byte-identical to the pre-change struct, so
   registry-only locks and the frozen declaration-hash corpus stay green.
   Members ride the unchanged registry expand/fetch/install path. Full
   design + phasing: [`plan_local_bundles_tui_group.md`](../state/plans/plan_local_bundles_tui_group.md).
6. **Dev-install = `grim install <path>`** writing a normal install record
   marked `dev: true`; `prune_orphans` never reaps dev records; `status`
   lists them, `update` re-packs + re-renders them on drift, `uninstall`
   removes them.
7. **Source drift maps to `ArtifactStatus::Outdated`** (not `Modified`,
   which already means installed-output drift). Remediation is identical to
   the registry case: `grim update <name>`.
8. **TUI gains a "Local" root group** (path declarations + dev records)
   riding the existing `TuiRow.source` non-OCI-root seam, with update and
   delete actions.
9. **`--watch` is out of scope** — change detection happens at command time
   (Cargo model). A watcher is a possible follow-up once the manual loop
   proves insufficient.

### Consequences

**Positive:**
- Local-first authoring loop: edit → `grim update` → rendered in every client.
- Repo-relative path deps make "clone + install" work for co-workers.
- All existing digest-keyed flows (lock idempotency, integrity gate, status
  derivation) work unchanged against content hashes.

**Negative:**
- Forward incompatibility: old grim cannot read path-bearing locks/state
  (exit 78) — documented in `stability.md`; triggers only when the feature
  is used.
- `status`/`update` re-pack local sources per invocation (cheap at artifact
  sizes; mtime-keyed caching is a noted ceiling).

**Risks:**
- Widest ripple is the `DesiredSet`/`ArtifactRef` re-type (~12 files incl.
  TUI) — mitigated by compiler-driven refactor + frozen declaration-hash
  corpus + lock byte-identity tests.
- Local-bundle provenance (`bundle_path`) touches effective-set eviction —
  covered by dedicated unit + acceptance tests.

## Technical Details

### Data Model

```
grimoire.toml value ── is_path_value? ──► DeclaredSource::Path(PathSource)
                                    └───► DeclaredSource::Registry(Identifier)

lock entry:   pinned = "reg/repo@sha256:…"        (Registry)
              path = "./x" + hash = "sha256:…"     (Path)      ← XOR

install rec:  same XOR + optional dev = true       (dev-install marker)
```

### API Contract (wire)

```toml
[[skill]]
name = "my-skill"
path = "./skills/my-skill"
hash = "sha256:…"          # SHA-256 of the canonical packed tar

[[bundle]]
name = "my-stack"
path = "./bundles/stack.toml"
hash = "sha256:…"          # SHA-256 of the canonical JSON members layer
```

## Implementation Plan

Phases 0–9 in the approved plan (core types → config ripple → lock wire →
resolve → install/status/update → add path form → dev-install → TUI Local
group → tests/docs/catalog). See plan artifact.

## Validation

- [ ] Registry-only lock/state byte-identity tests pass untouched
- [ ] Frozen declaration-hash corpus passes untouched
- [ ] `GRIM_OFFLINE=1` lock + install of tar-kind path deps succeeds
- [ ] Repack-twice-same-hash contract test
- [ ] `task verify` + `task catalog:verify`

## Links

- `handover_local_path_deps.md` — pre-ADR exploration
- `adr_effective_set_mutations.md` — bundle provenance / eviction model
- `adr_install_state_portability.md` — install-state V2 + anchors
- `adr_agent_artifact_kind.md` — kind-by-shape + `--kind` override precedent

---

## Changelog

| Date | Author | Change |
|------|--------|--------|
| 2026-07-11 | MH + Claude | Initial accepted version |
| 2026-07-11 | MH + Claude | Record local-bundle `LockedBundle` source-discriminant wire (sub-decision 5); link implementation plan for the two deferred v1 items (local bundles, TUI Local group) — implement, not defer |
