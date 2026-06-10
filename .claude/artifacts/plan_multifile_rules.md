# Plan: Multi-file rules (index `.md` + sibling support directory)

## Status

- **Plan:** plan_multifile_rules
- **Active phase:** 4 — Implementation
- **Step:** /swarm-execute → implementation
- **Last update:** 2026-06-10 (initial implementation + tests landed)

---

## Overview

**Status:** In Progress
**Author:** Builder
**Date:** 2026-06-10
**Related ADR:** [adr_multifile_rules.md](./adr_multifile_rules.md)

## Objective

Let a Grimoire **rule** optionally carry a sibling support directory of any
files, packed into the same single TAR layer and installed beside the index
file as `rules/<name>.md` + `rules/<name>/…`. A plain single-file rule stays
the degenerate case — no behavior change for existing rules.

## Scope

### In Scope

- Extend the existing `Rule` kind with an optional sibling support dir.
- Pack index + support files into one tar layer (reuse skill transport).
- Install the support dir beside the index for every client (verbatim copy;
  only the index is transformed for Copilot).
- Fold the index + support footprint into the integrity hash.
- Record the support dir in the install state; remove it on uninstall.

### Out of Scope

- Generic multi-path releases (rejected — loses index/support structure).
- Per-client relative-link rewriting (verbatim copy is the MVP).
- Multi-file *bundles* (bundles remain a JSON members layer).
- A new artifact kind (rejected — fragments the wire contract).

## Technical Approach

### Key Decisions

| Decision | Rationale |
|----------|-----------|
| Extend `Rule`, not a new kind / generic multi-path | Smallest change; OCI wire contract unchanged; clean name derivation + install destinations |
| Authoring input stays the index `.md` file | Keeps `detect_kind` unambiguous |
| Sibling dir detected as `file.with_extension("")` | Same stem as the index; included only when a real directory |
| Support files copied verbatim for every client | Only the index is transformed (Copilot); matches skills |
| Centralize the footprint hash; `ClientRecord::current_hash()` | One way to compute the two-root hash across all 5 integrity readers |
| `ClientRecord.support_dir: Option<PathBuf>`, keep `InstallStateVersion::V1` | Additive + optional ⇒ old records load unchanged |

## Implementation Steps

### Phase 4: Implementation (done)

- [x] **Packing** (`src/skill/skill_package.rs`): `pack_rule_file` emits the
  index `<name>.md` plus every file under a sibling `<name>/` dir (reusing
  `collect_files`, sorted ⇒ deterministic). Byte-identical without one.
- [x] **Materializer** (`src/install/materializer.rs`): already extracts
  multi-entry tars; covered by a new rule-specific test.
- [x] **Integrity hash** (`src/install/content_hash.rs`): add
  `footprint_hash(target, support_dir)` folding index (`<name>.md`) +
  support files (`<name>/<rel>`); `None` ⇒ identical to `content_hash`.
- [x] **Install record** (`src/install/install_state.rs`): add
  `support_dir: Option<PathBuf>` + `ClientRecord::current_hash()`; update
  `client_outputs()`.
- [x] **Per-client transform** (`src/install/client_target.rs`):
  `materialize` / `materialize_rule` copy the support dir verbatim to
  `<dest_parent>/<name>/`.
- [x] **Installer** (`src/install/installer.rs`): locate the staged support
  dir, pass it through, hash the combined footprint, record `support_dir`,
  remove prior support dir before replace.
- [x] **Uninstall** (`src/install/uninstall.rs`): reap the support dir
  alongside the index (`remove_output` helper), idempotent.
- [x] **Readers** (`status.rs`, `status_badge.rs`, `prune.rs`,
  `tui/app.rs`): switch integrity comparison to `out.current_hash()`.

### Phase 5: Review & Documentation (done)

- [x] ADR `adr_multifile_rules.md` + ADR-index row in `arch-principles.md`.
- [x] Docs: `concepts.md`, `publishing.md`, `subsystem-file-structure.md`,
  `subsystem-cli-commands.md`.

## Files to Modify

| File | Action | Description |
|------|--------|-------------|
| `src/skill/skill_package.rs` | Modify | Pack index + sibling support dir |
| `src/install/content_hash.rs` | Modify | `footprint_hash` over index + support |
| `src/install/install_state.rs` | Modify | `ClientRecord.support_dir` + `current_hash()` |
| `src/install/client_target.rs` | Modify | Copy support dir beside the index |
| `src/install/installer.rs` | Modify | Thread staged support dir + combined hash |
| `src/install/uninstall.rs` | Modify | Remove support dir on uninstall |
| `src/command/status.rs`, `src/install/status_badge.rs`, `src/install/prune.rs`, `src/tui/app.rs` | Modify | Use `current_hash()` |
| `test/tests/test_multifile_rules.py` | Create | Release → install → drift → uninstall → idempotency |

## Testing Strategy

### Unit Tests

| Component | Behavior | Edge Cases |
|-----------|----------|------------|
| `skill_package` | Packs index + support; identical without one; deterministic | No sibling dir |
| `materializer` | Extracts a multi-entry rule tar | — |
| `content_hash` | Footprint stable + location-independent; drift detected | `None` ⇒ legacy hash |
| `client_target` | Index transformed (Copilot) / verbatim; support verbatim | All clients |
| `installer` | Fresh → no-op → support drift refused → forced | — |
| `uninstall` | Removes index + support dir; idempotent | — |

### Acceptance Tests

| User Action | Expected Outcome |
|-------------|------------------|
| `grim release` a rule with a support dir, then `install` | `rules/<name>.md` + `rules/<name>/…` land |
| Edit a support file, `grim status` | `modified` |
| `grim uninstall` | Both index and support dir removed; idempotent |
| Re-release identical content | Same digest |

## Risks

| Risk | Mitigation |
|------|------------|
| Two-root footprint threaded through many seams | Centralized `footprint_hash` + `ClientRecord::current_hash()` |
| Old install records without `support_dir` | `#[serde(default)]` ⇒ load as `None`; `InstallStateVersion::V1` kept |

---

## Progress Log

| Date | Update |
|------|--------|
| 2026-06-10 | ADR accepted; implementation + unit + acceptance tests landed; `task rust:verify` green |
