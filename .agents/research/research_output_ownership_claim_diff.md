# Research: Claim diff and architecture map for ownership without install state

## Metadata

**Date:** 2026-11-15
**Domain:** cli
**Triggered by:** `/hex-architect` on `.agents/discussions/stale-outputs-without-state.md` (Phase 1 claim diff)
**Expires:** 2027-02-15 (code facts; re-read the named functions before relying on them)

## Direct Answer

Every code claim in the dossier holds, three of them with a correction that
changes the design. Adoption of a file output re-materializes identical bytes
and does not write the record itself. A `Removed` row for a dropped member with
no record exists only on a named run. Generated rule and agent files embed the
pin string, so a file written at the old pin matches only a preview rendered
with the old pin. Read in code on 2026-11-15 by one explorer; nothing was run.

## Claim diff

| # | Claim | Verdict | Evidence |
|---|---|---|---|
| 1 | `derive_state` with no record returns `missing`, disk never read | holds | `src/command/status.rs:2881-2883` |
| 2 | update order: previous lock, save lock, load state, install pass, prune | holds | `src/command/update.rs:136, 173, 200, 244, 311`. `previous` stays in scope to `:462`. |
| 3 | no record plus identical footprint is adopted; differing is `RefusedUntracked` (65) unless `--force` | holds, with a nuance | `src/install/installer.rs:1220-1297`. For files, adoption re-materializes identical bytes through the swap path (`:1330-1470`). "Writes no byte" is true only for MCP. |
| 4 | footprint hash walks all regular files minus the ignore set | imprecise | `src/install/content_hash.rs:100, 213-236`. Ignored: `DEFAULT_PATTERNS` (`src/install/ignore_set.rs:25`) and the destination's own `.grimignore`. Symlinks and empty directories are skipped, so those extras do not block adoption. A regular extra file does. |
| 5 | `prune_orphans` walks state records only | holds | `src/install/prune.rs:237-243` |
| 6 | dropped member with no record still gets a `Removed` row | imprecise | The test builds a named run (`src/command/update.rs:1657-1670`). For a bare run `build_report` drops `Dropped` rows (`:1383-1385`). |
| 7 | `ClientOutput` is `deny_unknown_fields`; `adopted` written only for MCP entries; uninstall keeps an adopted entry | holds | `src/install/install_state.rs:84-131`; file outputs write `adopted: false` (`installer.rs:1516`); MCP writes `plan.adopted` (`:3372`); keep rule at `src/command/uninstall.rs:279-285` and `installer.rs:2591` |
| 8 | `ArtifactStatus` literals | holds | `src/api/artifact_status.rs:22-70`. Eight literals, kebab-case serde: `installed`, `stale`, `modified`, `missing`, `outdated`, `gated`, `not-armed`, `untrusted`. No untracked-like literal. |
| 9 | `destination` follows from (client, kind, name) with no content | holds | `src/install/expected_outputs.rs:296-310`. It also takes the record, for the skill pool peer set. |
| 10 | `test_clobber_guard.py` coverage | holds | Refusal, `--force` and adoption at the same pin, for skill and MCP (`test/tests/test_clobber_guard.py:44, 70, 90, 124, 153`). `rmtree(".grimoire")` at `:104, 165`. No test of update with no state. |

## Residual ground

### The adopt branch

- Inputs, all inside `install_one` (`installer.rs:996`): the staged tree and
  `canonical` from `stage_locked_artifact` (`:2712`); the per-client preview
  from `client.materialize(MaterializeRequest{…, pinned, support_dir})`
  (`:1268-1286`); `footprint_hash_for_kind` on the preview and on the
  destination (`:1287-1290`); `destination_among` with
  `effective_clients(.., None, ..)` (`:1217`, `expected_outputs.rs:272, 322`);
  `AnchoredPath::from_target`.
- It takes the lock entry only for `name`, `kind` and `source.provenance()`.
  That string is embedded in the provenance header of generated files
  (`src/install/vendor.rs:1462-1476`), so an old-pin file matches only a
  preview rendered with the old pin.
- Nothing in the gate is tied to "the entry being installed", but it is
  inline, not a function. A pre-pass would re-implement the loop `:1186-1299`,
  apply the `client_supports_kind` / `name_fits` filters (`:1137-1160`), build
  `ClientOutput` (`:1507-1517`) and call `state.record`
  (`install_state.rs:1081`).
- File-branch adoption does not write the record itself. The later
  `state.record` at `:1670` does.

### Old content

- Path: `stage_locked_artifact` → `fetch_verified_layer` →
  `access.fetch_manifest(pinned)` → `check_locked_edges` → `fetch_blob`
  (`installer.rs:2712-2860`).
- `CachedAccess` reads the store first in both modes
  (`src/oci/cached_access.rs:100-115, 165-183`). An offline miss is
  `OfflineMiss` (`:113, 178`), which maps to `OfflineBlocked`
  (`error.rs:564`), exit 81.
- A no-network read already exists: `ctx.access_with_mode(AccessMode::Offline,
  vec![])`, used by status (`status.rs:715-718`). `BlobStore::has` exists but
  is `dead_code` (`blob_store.rs:45`). `cached_mcp_descriptor` reads the store
  directly (`expected_outputs.rs:104`).

### State write path

- No single seam. `install`, `add`, `install --dev` and the TUI share
  `install_and_persist` (`installer.rs:757`; `src/command/install.rs:107,
  408`; `src/command/add.rs:1286`).
- `update` loads state itself (`update.rs:200`) and persists itself.
- `uninstall` loads state only when `!held_by_bundle`
  (`src/command/uninstall.rs:129`).
- `remove` loads no state (`src/command/remove.rs:123`).
- Every site goes through `scope_resolution::load_state`
  (`scope_resolution.rs:313`). Records are written by `InstallState::record`
  and saved by `persist` (`install_state.rs:1325`), which creates `.grimoire/`
  and its `.gitignore`.

### Bundles

- Members are expanded into ordinary lock entries, each stamped with
  `bundles: Vec<BundleProvenance>` (`src/lock/locked_artifact.rs:67`).
  `lock.bundles[].members` is also cached (`locked_bundle.rs:85`). The previous
  lock alone lists the old members.
- Dropped members are computed by `plan_lock` (`disposition.rs:39`), called at
  `resolver.rs:232`. `prune_orphans` re-derives orphans from state versus the
  new lock.

### Named update

- `NamedScope::of` (`update.rs:698`) takes `install` as the new-lock closure
  and `prune` as the previous-lock closure, through `closure_nodes`
  (`closure.rs:221`).
- Prune takes `only = n.prune` (`update.rs:310-311`; `prune.rs:243`). The
  install pass receives `project_lock(&new_lock, &n.install)` (`update.rs:231`).

### Global scope

One code path. The only difference is the state location: `load_global`,
`$GRIM_HOME/state/global.json` (`scope_resolution.rs:318`, `:132`). No scope
branch was found in the gate or in prune. The global branches at
`installer.rs:630-709, 1356` concern hooks and the Copilot root.

### Status plumbing

- `derive_state` takes `(kind, name, locked, state, roots, active,
  lock_matches_config)` and has no store access. A store-only access seam
  exists at `status.rs:715`.
- `outputs_pending` comes from `pending_outputs` (`expected_outputs.rs:79`).
  With no record `is_covered` is false (`:257`), so it already lists every
  expected destination (inferred).
- The `state` vocabulary is documented as five literals
  (`docs/src/content/docs/json-interface.md:137`), the row shape at `:96`.
  Both are governed by `docs/src/content/docs/stability.md:34, 42, 50-60`.
- A new literal touches the exhaustive match at `src/tui/app.rs:1951-1963`.
  The enum is also referenced in `status_badge.rs`, `settings_phase.rs`,
  `hook_report.rs` and `hook/list.rs`.

### MCP entries and generated files

- MCP: `existing_value == value`, after stripping vendor-owned keys, adopts
  with `adopted = true`. A differing value gives `RefusedUntracked`
  (`installer.rs:3223-3271`). `tracked` needs a recorded output with the same
  anchored pair, pointer and value hash (`:3242-3249`), hashed by
  `entry_value_hash`.
- Generated rule and agent files are judged by the footprint hash of a preview
  rendered with the pin string.

### Test fixtures

- `project_dir`, `grim_at`, `grim_home` (`test/conftest.py:30, 105`).
- `make_artifact(repo, kind, files, tag=)` and `make_bundle(repo, members,
  tag=)` (`test/helpers.py:341, 118`); retag is supported per the docstring
  (`:352`).
- `--offline` runs: `test/tests/test_status.py:285`. Offline fetch exit 81:
  `test/tests/test_fetch.py:370`.
- A cold store needs a second `GrimRunner` with a fresh home (inferred from
  `conftest.py:105-119`; no existing test was found that does this).

## Prior decisions that constrain the design

1. **Adopting recordless files by content hash** is not prohibited.
   `.agents/adr/adr_install_state_portability.md` (Accepted): "Cannot
   distinguish grim-managed from hand-placed files without a hash match"
   (`:180`); the stateless option is not foreclosed (`:202-205`).
   `.agents/adr/adr_render_layout_stability.md:112-124` (Accepted): vendor
   layout is outside the semver contract, so a newer grim can render different
   bytes and the hash will not match. That is a refusal, not a defect.
2. **Removal without a prior record.**
   `.agents/plans/bugfix_plan_reaper_ownership.md:42` (Complete): "grim
   removes only elements it can prove it wrote." `:79`: a filesystem heuristic
   substituted for ownership evidence was the root cause. `:176`: a probe
   "cannot distinguish grim's element from an identical one the user typed."
3. **Previous lock as evidence.**
   `.agents/adr/adr_effective_set_mutations.md:483-490` (Accepted) derives
   file-deletion targets from lock entries, but only where records exist.
   `.agents/adr/adr_lock_pin_stability.md:183` (Accepted) keeps "install never
   prunes" and "remove never deletes".
4. **Fetching an old pin.** `adr_effective_set_mutations.md:42` makes `remove`
   and `uninstall` offline-only. `adr_lock_pin_stability.md:159`: a carried pin
   whose manifest is gone fails with 79 and only `update <name>` may move it.
5. **Repository-delivered evidence.** `.claude/rules/arch-threat-model.md:84-89`,
   invariant I1 (T3): a record that travels in a repository is
   attacker-suppliable, and `git pull` replaced a gitignored `state.json`. The
   lock is repository-delivered. A hash match against pinned content is not a
   record, but this is the constraint to answer.
6. **New status literal.** `stability.md:50-60`, `:42` and
   `.claude/rules/subsystem-cli-api.md:68`: additive only. The literal
   precedent is `kept-by-dependency` (`stability.md:139`). Older consumers must
   ignore unknown values.
7. **An adopted flag on a file output** is a state-schema break:
   `install_state.rs:121-130`, an older grim rejects the field.

## Record pin, lock pin and the files: what the code does today

Read on 2026-11-15 by a second explorer, after the owner asked whether the
record should be keyed by the locked version or stored in the lock.

- **A record already names its pin.** `InstallRecord.source` is a
  `LockedSource` with registry, repository and digest
  (`src/install/install_state.rs:565-578`, `src/lock/locked_source.rs:41-56`).
  The pin is per record; a `ClientOutput` holds none (`:86-139`).
- **Status compares it.** Intact files plus an equal pin read `installed`;
  intact files plus a different pin read `outdated`
  (`src/command/status.rs:2889`, `docs/src/content/docs/commands.md:1584`).
  `stale` is unrelated: the lock no longer matches the config (`:2861-2873`).
- **The hash check runs first.** `integrity_gate` refuses when a recorded
  output's hash differs from the file, before any pin comparison
  (`src/install/installer.rs:1988-2003`; the pin is compared only at `:2035`).
  Status tests `modified` before `outdated` (`status.rs:2884-2890`). So a record
  at pin A, a lock at pin B and files that are a correct render of B refuse
  with 65 and read `modified`. The code cannot tell that case from a hand edit.
- **A pin mismatch is also the normal update flow.** During `grim update` the
  record at A and the lock at B is expected; `pin_changed` re-materializes and
  `reap_moved_outputs` reads the old record (`installer.rs:1091`, `:2098`).
  With gitignored outputs after a branch checkout the same mismatch is correct
  too: the record describes the disk and the lock is ahead. A rule that voids
  the record on a pin mismatch would break both.
- **The stale record is still needed.** Later code reads its outputs for the
  peer set (`src/install/expected_outputs.rs:272-295`), the pin-change
  carry-forward (`installer.rs:1093`, `:1624-1640`), the layout reapers
  (`:2098`, `:2475`), prune and dropped clients (`src/install/prune.rs:201`,
  `:480`), preserved unselected clients (`installer.rs:1902`), and the MCP
  `adopted` flag.
- **The lock cannot carry the mapping without a break.** Every lock table is
  `deny_unknown_fields` (`src/lock/grimoire_lock.rs:29,92,128`,
  `src/lock/locked_artifact.rs:16,80`, `src/lock/locked_bundle.rs:147`), first
  introduced in a commit that is in v0.1.0. An older grim exits 78 on an unknown
  table or field (`docs/src/content/docs/stability.md:753-761`).
  `lock_version` is a closed enum with `V1` only.
- **The client set is partly per machine.** Order: `--client`, then
  `[options].clients` in `grimoire.toml`, then the global list joined with
  detected clients, then `agents` (`src/install/target.rs:166-200`). Two
  teammates on one commit can produce different output sets unless the project
  file pins the clients.
- **Committed state is already read and sanitized.** Project load drops
  settings records and every output whose path is not a legal surface for its
  kind and client (`install_state.rs:1018-1040`, `:1620-1650`, `:1730`).
- **A hash from disk needs no render.** `footprint_hash` and
  `footprint_hash_for_record` (`src/install/content_hash.rs:88-131`).

## Not determined

- No eviction code was found in `src/store`; `src` was not searched
  exhaustively.
- Whether a pre-pass before the lock save is needed. `previous` outlives the
  save (inferred).
- Whether a bare-run `Removed` row is wanted. Today only named runs produce it.
