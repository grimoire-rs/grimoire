# Bug Fix Plan: grim deletes `claudeMdExcludes` elements it never wrote

## Status

- **Plan:** bugfix_reaper_ownership
- **Active phase:** 5 — Verify
- **Step:** finalized
- **Last update:** 2026-08-25 (after c4916c1: fix(install): exclude a rule's Claude support directory from auto-load)

---

## Overview

**Status:** Complete
**Author:** builder (hex/reaper-ownership)
**Date:** 2026-08-25
**GitHub Issue:** follow-up to [grimoire-rs/grimoire#102](https://github.com/grimoire-rs/grimoire/issues/102)
**Severity:** High — silent data loss in a consumer's git-tracked file

## Bug Report

### Observed Behavior

`claude_config::sync_for_state` removed **any** managed-shape `claudeMdExcludes`
element that (a) matched grim's spelling for the scope, (b) was wanted by no
install record, and (c) whose `<rules_dir>/<name>/` was absent from disk.

Verified against a real consumer's committed `.claude/settings.json` (two
hand-written exclusions plus `permissions` and `hooks`): with the trees absent
from disk, grim deleted the entire `claudeMdExcludes` key. That project
survives today only because it happens to commit its support trees; a project
that gitignores them loses both lines on a fresh clone, and the deletion lands
in someone's commit.

Second defect from the same guess: grim's element is tree-wide
(`**/.claude/rules/<name>/**`) while the staleness probe checked exactly one
location, so a monorepo entry covering `packages/*/.claude/rules/shared/` was
deleted too.

### Expected Behavior

grim removes only elements it can prove it wrote. A consumer's own exclusion —
for a rule grim never had a record of — survives every sync.

### Reproduction Steps

1. Write a `.claude/settings.json` holding `claudeMdExcludes: ["**/.claude/rules/mine/**"]`.
2. Ensure `.claude/rules/mine/` does not exist (gitignored support tree, fresh clone).
3. Run any command that reaches the Claude vendor config sync with no matching record.
4. The element — and the emptied key — is gone.

Pinned as the unit reproduction
`claude_config::tests::a_user_authored_element_for_a_rule_grim_never_recorded_survives`,
which failed on the pre-fix code with `left: "{\n}\n"`.

### Environment

| Factor | Value |
|--------|-------|
| Platform | any |
| Grimoire version | branch `fix/claude-md-excludes` @ 6e26cdc (unreleased) |
| Configuration | project or global scope, both affected |

### Frequency

Always, whenever the directory is absent and no record wants the name.

## Root Cause Analysis

### Investigation Log

1. **Symptom**: a user-authored `claudeMdExcludes` element disappears from a
   git-tracked `settings.json`.
2. **Proximate cause**: `stale_elements` (`src/install/claude_config.rs`)
   returned every managed-shape element whose directory was absent.
3. **Root cause**: `Vendor::sync_config` received only **post-mutation** state.
   By the time the sync ran, an uninstalled rule's record — and its name — was
   already gone, so the module had no way to name what it should deregister and
   substituted a filesystem heuristic for ownership evidence.
4. **Introduced by**: the original `claudeMdExcludes` implementation on this
   branch (never released).

### Root Cause Statement

> grim deletes exclusions it never wrote because `sync_config` is handed only
> post-mutation state and cannot name what went away, so removal was inferred
> from a directory's absence — a signal that cannot distinguish grim's element
> from an identical one the user typed.

### Related Code

| File | Role |
|------|------|
| `src/install/vendor.rs` | `Vendor::sync_config` — the seam missing the removal evidence |
| `src/install/claude_config.rs` | `stale_elements` — the filesystem guess (deleted) |
| `src/install/install_state.rs` | `retired_outputs` — the new evidence source |

### Pattern Check

- [x] Searched for the same defect in the sibling vendor: `opencode_config`'s
      managed `instructions` glob is a **single fixed value** keyed off state
      alone (present iff any OpenCode rule is recorded), so it has no per-name
      removal to guess and is unaffected.
- [x] Regression from a recent change? No — original implementation on this
      unreleased branch.
- [x] Other callers of the root cause? All six `sync_config` call sites were
      passing post-mutation state only; all six now supply `retired`.

## Regression Test Specification

### Unit Tests

| Test | File | Asserts |
|------|------|---------|
| `a_user_authored_element_for_a_rule_grim_never_recorded_survives` | `src/install/claude_config.rs` | The regression: an element with no record and an absent directory is preserved |
| `a_real_consumer_settings_file_is_untouched_byte_for_byte` | `src/install/claude_config.rs` | Real-shaped fixture (2 hand-written exclusions, `permissions` with `"// === … ==="` divider strings, `hooks`) is byte-identical after a sync with empty state and empty `retired` |
| `uninstall_removes_the_exclusion_and_drops_the_emptied_key` | `src/install/claude_config.rs` | Existing behaviour preserved — the element goes, no `[]` husk |
| `uninstalling_one_of_two_rules_leaves_the_survivors_element` | `src/install/claude_config.rs` | Only the retired rule's element goes |
| `a_retired_output_for_another_client_removes_nothing` | `src/install/claude_config.rs` | A non-Claude retired output contributes no removal |
| `a_retired_output_whose_name_is_not_an_artifact_name_removes_nothing` | `src/install/claude_config.rs` | `SkillName::parse` gates the removal side too (warns, removes nothing) |
| `a_name_that_is_both_retired_and_wanted_keeps_its_element` | `src/install/claude_config.rs` | `wanted` wins over `retired` |
| `an_update_that_drops_the_support_dir_deregisters_it` | `src/install/claude_config.rs` | A surviving record whose new version dropped its support dir still loses its element |
| `a_settings_path_that_cannot_be_written_fails_the_removal` | `src/install/claude_config.rs` | An unreadable settings file propagates rather than silently converging |
| `retired_outputs_reports_every_shape_of_removal` | `src/install/install_state.rs` | Dropped record, reaped sibling, replaced output all retire; verbatim survivor and fresh record do not |

### Acceptance Tests

None added. The two shipped end-to-end assertions
(`test_multifile_rules.py`, `test_global.py` — uninstall drops the key at both
scopes) already exercise the `grim uninstall` path through the new evidence
route and stay green.

## Fix Approach

### Proposed Change

Give `sync_config` the information it was missing instead of making it guess.

1. **Widen the trait seam** — `Vendor::sync_config` gains
   `retired: &[ClientOutput]`: the outputs the triggering operation removed
   from the install state. Vendor-agnostic (every client's outputs, not just
   this vendor's), empty on a pure install.
2. **One evidence source** — `install_state::retired_outputs(before, after)`
   diffs a pre-mutation snapshot against the post-mutation state. An output is
   retired when the post-state's record for the same `(kind, name)` does not
   hold it verbatim, which covers every operation shape in one place: a dropped
   record (uninstall, pruned orphan), an output reaped from a surviving record
   (dropped client), and a replaced output (a new version that stopped shipping
   a support directory).
3. **Rewrite the Claude removal side** — `wanted` is unchanged (state-derived);
   `stale` is derived only from `retired`, minus anything still in `wanted`.
   `stale_elements` and its filesystem probe are deleted, along with
   `managed_name` (grim no longer reads elements back — removal matches the
   spelling it recomputes).

### Files to Modify

| File | Change |
|------|--------|
| `src/install/install_state.rs` | New `retired_outputs` free function + its test |
| `src/install/vendor.rs` | `sync_config` gains `_retired: &[ClientOutput]` |
| `src/install/vendor_claude.rs` | Override forwards `retired` |
| `src/install/vendor_opencode.rs` | Override ignores `retired` (documented why) |
| `src/install/claude_config.rs` | `stale` from `retired`; `stale_elements`/`managed_name` deleted; shared `support_dir_names` helper |
| `src/install/installer.rs` | Snapshot before install; supply `retired` |
| `src/command/uninstall.rs` | Snapshot before mutation; supply `retired` |
| `src/command/update.rs` | Snapshot before install/prune/reap; supply `retired` |
| `src/tui/app.rs` | Same at all three delete seams |
| `.claude/rules/subsystem-file-structure.md`, `docs/src/vendor-metadata.md` | Removal is record-driven, not a filesystem probe |

### Alternatives Considered

| Approach | Rejected Because |
|----------|-----------------|
| Record the element as a `ClientOutput` with an `entry` JSON pointer | A state-schema change: it needs migration, an old-path reaper, and an upgrade fixture, and it puts a field an older grim rejects into every state file — all to carry information a parameter already has. Principle 9 exposure for nothing. |
| Keep the filesystem probe, narrow it | The probe cannot distinguish grim's element from an identical one the user typed, at any width. Narrowing changes how often it destroys the user's line, not whether. |
| Thread the removed outputs out of `prune.rs`'s `delete_output` | Only reaches two of the paths that retire an output (see Notes) and needs new fields on `PrunedArtifact`/`ReapedClients` plus signature changes. The state diff reaches all of them in one function and adds no field anywhere. |

### Risk Assessment

| Risk | Mitigation |
|------|------------|
| A path that retires an output without supplying `retired` leaves a stale element | The state diff is computed at every `sync_config` call site from a snapshot taken before that command's first mutation, so it cannot miss a mutation that command made |
| The `wanted`/`retired` intersection wrongly keeps an element | Pinned by `a_name_that_is_both_retired_and_wanted_keeps_its_element` and `an_update_that_drops_the_support_dir_deregisters_it`, which sit on opposite sides of that rule |
| A stale element grim can no longer reach (written before a `rules/` segment move) | `the_managed_prefix_and_the_vendors_rule_path_share_one_rules_segment` pins the two spellings together |

## Verification Checklist

- [x] Regression test failed on current code (`left: "{\n}\n"` vs the untouched file)
- [x] Fix applied — regression test now passes
- [x] All existing tests still pass (`task verify`)
- [x] No `[]` husk, no scope regression: `test_multifile_rules.py` / `test_global.py` green
- [x] No scope creep — no state-schema change, no `ClientOutput` field, no `entry` machinery, `**/` glob spelling untouched

## Notes

**The design brief's `prune.rs` premise did not hold.** `delete_output` is
*not* the single per-output deletion seam for both update paths: only
`reap_dropped_clients` calls it, while `prune_orphans` deletes through
`uninstall::uninstall`. More importantly, neither path sees the third way an
output is retired — a rule re-installed at a new version that dropped its
support directory replaces its own record through `installer.rs`, which the
brief expected to pass `&[]`. Passing `&[]` there would have left that rule's
exclusion behind forever and regressed the shipped
`an_update_that_drops_the_support_dir_deregisters_it` behaviour. The state
diff covers all three shapes and needs no change to `prune.rs` at all.

**Documented, accepted semantic:** where a consumer hand-wrote the exact
element grim would write, the upsert adopts it silently (grim cannot
distinguish "I wrote this last run" from "the user typed it") and removes it
when that rule is uninstalled. Convergent and correct — the line's only purpose
was to exclude that rule's tree, and the tree is going away with it. Recorded
in the `claude_config` module doc.
