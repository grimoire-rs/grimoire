# Bug Fix Plan: transitive dependencies and nested bundles (0.16.0 follow-ups)

## Status

- **Plan:** bugfix_transitive_followups
- **Active phase:** 7 — Commit & Document
- **Step:** finalized
- **Last update:** 2026-11-18 (fixes, docs and review round landed on the branch)

---

## Overview

**Status:** Fixed on `fix/transitive-followups`, awaiting merge
**Date:** 2026-11-18
**GitHub Issue:** [#371](https://github.com/grimoire-rs/grimoire/issues/371)
**Severity:** High (one defect), Medium and Low (the rest)

grim 0.16.0 shipped artifact dependencies (`requires`) and nested bundles.
Several commands still assume that a bundle has flat members and that nothing
requires anything. Issue #371 reports the TUI symptom. A hunt over the rest of
the CLI found 15 more defects of the same family. Each was reproduced against
the 0.16.0 binary before this plan was written.

## Root cause

One pattern, repeated: code iterates `GrimoireLock::iter_artifacts()` (which
never yields bundles), or reads only the declared set, and so misses nested
bundles (`[[bundle]] transitive = true`) and dependency-only artifacts
(`transitive = true`, reached through `[[<kind>.requires]]`).

## Defects and fixes

| ID | Severity | Defect | Fix site |
|---|---|---|---|
| 371-a/b | medium | TUI: a nested bundle reads `not-installed`, the bundle declaring it reads `pending` | `src/tui/app.rs` `derive_bundle_state` |
| 371-c | medium | TUI: a dependency-only artifact reads `via-bundle` | `member_display_state`, new TUI state `via-dependency` |
| 371-d | low | TUI: a catalog row with no kind is looked up as a skill | `row_kind` |
| 1 | high | `grim uninstall` stops half-done on a dependency's hand-edited MCP entry | `src/command/uninstall.rs`: call `delete_refusals` before the first write |
| 2 | medium | `grim install` exits 0 on a lock with removed `requires` rows once installed | `src/install/installer.rs` `refuse_stale_edges` |
| 3 | medium | `uninstall` reports `kept-by-bundle` or names no bundle when a dependency or a nested bundle holds the artifact | `plan_hold`, `kept_by_bundle_note` |
| 4 | medium | `uninstall` of a parent deletes a locally edited dependency file without a word | warning through `prune::is_modified` |
| 5 | medium | `grim why bundle <name>` never reports overrides or the redundancy note | `src/command/why.rs` `direct_edges` |
| 6 | low | `status --check` promises an update that `grim update` refuses | note wording in `src/command/status.rs` |
| 7 | low | `grim remove` is silent about a dependency hook that stays armed | `src/command/remove.rs` |
| 8 | low | TUI: no update marker on bundle-provided and dependency rows | `eligible_for_recheck`, `mark_outdated_if_installed` |
| 9 | low | TUI: cursor jump on nested expand, "Via bundle:" label for a `requires` parent, no Pending count in the group summary | `state.rs`, `detail.rs`, `render.rs` |
| 10 | low | `add.pulled_in` never lists nested bundles | `src/command/add.rs` `Added::new` |
| 11 | medium | A repository republished as another kind is saved under the old kind; every later install exits 65 | `src/resolve/graph.rs`: compare `kind_from_manifest` |
| 12 | medium | TUI Install on a bundle member fails with a namespaced `[[registries]]` entry | `perform_member` and its identifier helpers |
| 13 | low | `grim search` and MCP `grim_search` show every bundle `not-installed` | `src/install/status_badge.rs` |
| 14 | low | `grim add` arms hooks that the next `grim install` disarms when another declared hook is unconsented | `src/command/hook_consent.rs` `gesture` |
| 15 | low | `state.json` pins carry the tag after `add`/`update`, not after `install` | record construction in `installer.rs` |

## Rulings

1. A dependency-only artifact reads `via-dependency` in the TUI; a nested
   bundle reads `via-bundle`; the declaring bundle reads `installed`.
2. Uninstalling a parent still deletes a locally edited dependency file (the
   reference says materialized files are deleted either way) and now warns.
   Keeping such files is the subject of
   [#353](https://github.com/grimoire-rs/grimoire/issues/353).
3. `grim install` checks the edges of installed entries too, from the local
   store only, never through the network.
4. `grim add` follows the install rule for hooks: nothing arms while a
   declared hook lacks consent. This replaces the sentence "those hooks stay
   armed" in the `add` reference. The TUI Install and Update share the code
   and follow the same rule; global scope keeps no record and is unaffected.
5. No new JSON key, flag or exit code. Defect 11 adds one error reason.

## Execution

Five lanes, at most three at a time, each in its own worktree under
`.agents/worktrees/tf-<lane>` with one shared build directory. Issues that
share a file run in sequence inside a lane. Workers write the regression test
first, hand back one patch per issue and never commit. Patches land here, one
`task verify` per batch, one `fix:` commit per defect.

| Lane | Files | Defects |
|---|---|---|
| T | `src/tui/**` | 371-a to d, 12, 8, 9 |
| U | `src/command/uninstall.rs`, `src/install/uninstall.rs`, `src/install/prune.rs` | 1, 3, 4 |
| I | `src/install/installer.rs`, `src/resolve/**` | 2, 11, 15 |
| R | `src/command/{why,status,remove,add}.rs`, `src/install/status_badge.rs` | 10, 7, 13, 6, 5 |
| H | `src/command/hook_consent.rs` | 14 |

Then: one docs pass, one review round, `/finalize`, pull request,
`verify-basic`, `verify-deep`.

## Verification

- Each defect has a regression test that failed before its fix.
- `task verify` on the final branch.
- By hand: declared bundle `suite` nests `pack`, skill `gamma` requires rule
  `beta`; `grim tui` shows `suite` `installed`, `pack` `via-bundle`, `beta`
  `via-dependency`.
- CI: `verify-basic` on the pull request, `verify-deep` dispatched on the
  branch.

## Review round

One round: an opus security read (defects 2, 11, 14), two reviewers by area, a
Codex read and a real run of the reference scenario. Fixed from it: no update
check was built for a dependency-only TUI row; `add.pulled_in` missed a bundle
reached through `requires`; a false "not consented" warning at global scope
and under `--trust-workspace`; one install-record site kept the tag.

Left as found, both older than 0.16.0's dependencies:

- The TUI judges a bundle member by the repository the bundle lists. A member
  a direct declaration overrides with another repository reads `pending`.
- `uninstall` names the declared bundle that holds an artifact, not a second
  holder that reaches it through a required bundle.

## Not covered

- Why #371 looked different on Windows and Linux.
- TUI batch actions and the version picker, settings packages as
  dependencies, marketplace parts, resolver size caps.
