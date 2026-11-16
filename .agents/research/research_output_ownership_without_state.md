# Research: Ownership of rendered outputs without install state

## Metadata

**Date:** 2026-11-15
**Domain:** cli
**Triggered by:** `.agents/discussions/stale-outputs-without-state.md`
**Expires:** 2027-02-15 (code facts; re-read the named functions before relying on them)

## Direct Answer

With no `.grimoire/state.json`, grim proves ownership only by content: the
untracked-destination gate renders the locked version, hashes it, and adopts a
destination whose whole-tree footprint is identical. Removal is record-driven
everywhere, so a destination that was never adopted is never removed.
Everything below was read in code or docs on 2026-11-15; nothing was run.

## Key findings

- **Status** (`src/command/status.rs`, `derive_state`): no record returns
  `missing`; disk is not inspected. The status enum has no untracked literal.
- **Install** (`src/install/installer.rs`, the gate that returns
  `RefusedUntracked`): identical footprint is adopted and a record is written;
  a differing footprint is refused, exit 65, unless `--force`.
- **Footprint hash** (`src/install/content_hash.rs`,
  `footprint_hash_for_kind`): walks every regular file under the destination
  minus the `.grimignore` set, so an extra hand-placed file inside a skill
  directory changes the hash and blocks adoption.
- **Update** (`src/command/update.rs`): the previous lock is loaded, the new
  lock is saved, then the install pass runs against the new version, then
  `prune_orphans` (`src/install/prune.rs`) walks state records. The previous
  lock stays in scope (it feeds `NamedScope` and `record_changed`) but is not
  used as ownership evidence.
- **Report gap**: a bundle member dropped by the new version and holding no
  record still gets an `UpdateAction::Removed` row (unit test
  `a_dropped_member_with_no_record_keeps_its_removed_row`), while its files
  stay on disk and nothing warns.
- **What a lock entry gives** (`src/lock/locked_artifact.rs`): name, kind,
  pinned source, bundle provenance, `requires`. Destination paths follow from
  `(client, kind, name)` in `src/install/expected_outputs.rs` without reading
  content. Bundle membership is cached in the lock. The files inside a skill
  directory and a rule's support directory need the artifact content.
- **Old content availability**: the content store is append-only and no
  eviction code was found, so a warm machine likely still holds the old pin
  (inferred). A fresh machine holds nothing; offline against a cold store
  exits 81.
- **Markers today**: skill directories, Claude rule files and rule support
  directories are verbatim copies with no marker. Generated rule and agent
  files for some clients carry a provenance comment with the pin
  (`src/install/vendor.rs`). MCP entries carry none and are judged by value
  hash. Claude hook registrations carry `com.grimoire.managed`.
- **State schema**: `ClientOutput` uses `deny_unknown_fields`, so a new field
  that is written non-empty makes the file unreadable for an older grim.
- **Gitignore writes**: only `.grimoire/.gitignore` (`*`, `create_new`) in
  `src/install/install_state.rs`. No code writes the root `.gitignore` or
  `.git/info/exclude`; `.claude/rules/subsystem-install-state.md` states the
  root file is never touched.
- **Hooks and settings**: payloads, dispatch and records live under
  `$GRIM_HOME`, so a clone carries no stale hook or settings output.

## Earlier decisions that bind this topic

- `.agents/adr/adr_install_state_portability.md`: state in the lock was
  rejected because install facts churn a committed file and conflict across
  teammates; a stateless design was rejected for the orphan gap and because
  it "cannot distinguish grim-managed from hand-placed files without a hash
  match", with the note that it is not foreclosed.
- `.agents/plans/bugfix_plan_reaper_ownership.md`: "grim removes only elements
  it can prove it wrote"; removal is record-driven, never a filesystem probe.
- `.claude/rules/arch-threat-model.md`: a repository-delivered record is
  attacker-suppliable (a committed `state.json` plus payload armed a hook on a
  fresh machine), and `git pull` was shown to replace a gitignored state file.

## History of the ownership rule

Read from commit messages and issue titles on 2026-11-15.

- `e86b9118` introduced the untracked-destination gate after a first install
  "silently overwrote hand-authored files (and uninstall later deleted them)".
  Its adopt-if-identical branch was written for "the 'state file lost, outputs
  intact' case". No plan or ADR records alternatives; adopting against a
  different version, the lock as evidence, and markers were never written up.
- `d184d06a` added the content check to the tracked branch: "The pair proves a
  record NAMES the path; it does not prove grim wrote the bytes."
- `cfd25986` added the `adopted` flag so uninstall keeps an adopted MCP entry.
  `3db0c12e` stopped writing it for file outputs because no reader used it and
  an older grim rejects the field. An adopted file is therefore removed on
  uninstall like any owned file.
- `340184a9` (a `grim update` run in this repository, with state present)
  deleted the files a newer bundle version dropped, so stale removal works when
  records exist.
- `.agents/specs/design_render_scheme_versioning.md` (deferred) assumes a
  record exists and does not address a missing one.
- Open issues that touch the same limit: grimoire-rs/grimoire#167 (an output
  absent at the current root is dropped from the record and nothing is
  deleted), #97 (a reap keyed on directory existence was withdrawn as
  unsound), #295 (reports do not name an abandoned adopted MCP entry), #257.
  No issue reports `grim update` leaving stale files after a clone.

## Documentation conflict

- `docs/src/content/docs/guides/team-ci.md`: "Two files go into version
  control, and nothing else".
- `docs/src/content/docs/guides/shared-skills.md`: "Commit the declaration,
  not the copies", with a `.gitignore` listing the client directories, and a
  contributor runs `grim install` after cloning.
- This repository commits its rendered outputs (108 tracked files under
  `.claude/skills`), and `docs/src/content/docs/guides/use-a-marketplace.md`
  describes committed files as what `grim install` writes for a project.

## Not determined

- Whether any blob eviction exists outside `src/store`.
- How this repository's CI treats a clone with no state file.
