# Discussion: Stale outputs when install state is absent

State: handed-off → architect · Updated: 2026-11-15
Ratified: 2026-11-15 → architect
Confidence: the owner ratified by running `/hex-architect` on this artifact after the restate (2026-11-15); both research artifacts are dated 2026-11-15.

## Intent

Owner's words: "on checkout from git the install state maybe pending (which is
itself kinda wrong) but if i update without installing, while its pending, how
are files that maybe not shipped with newer versions deleted?"

Outcome shape: an ADR (owner, 2026-11-15).

Why now: every repository that commits its rendered outputs hits this on each
fresh clone or worktree, this repository included, and the 1.0 freeze makes a
later change to status literals or adoption behaviour harder.

Out of scope:

- The design of issue 257 (where ignore entries are written, its flag or
  config key, the reversal of "never touch the root `.gitignore`"). It stays
  an opt-in and gets its own plan.
- Any travelling ownership record: committed state, sidecar manifests, markers
  in rendered files.
- Hooks and settings. Their payloads and records live under `$GRIM_HOME`, so a
  clone carries no stale output of those kinds.
- A `grim status` that writes.

## Requirements

- Ownership is proven by content only. A destination that fails the hash
  match (hand-edited, extra file inside a skill directory, different version)
  is never adopted and never removed by name. The existing refusal (exit 65,
  forceable) for a differing destination the new version still ships stays.
- Nothing is pruned that was not adopted or already recorded.
- A report row must not say `Removed` for a file grim left on disk. Leftovers
  that could not be proven are named in a warning.
- No repository-delivered ownership record: no committed state file and no
  marker inside or beside rendered outputs. Rendered bytes stay unchanged.
- State and report changes are additive: the new status literal is appended,
  no existing literal changes meaning for a destination that has a record.

## Decisions

- Both checkout models stay supported. Committed outputs remain the default
  and get a real fix for the no-state case; issue 257's gitignore flag is an
  opt-in on top (owner, 2026-11-15).

- Adoption happens in mutating commands only (`install`, `update`,
  `uninstall`, `remove`): before acting, grim adopts every destination with no
  record whose whole-tree footprint hash-matches the lock as found on disk.
  `update` adopts against the previous lock before it relocks. `grim status`
  never writes (owner, 2026-11-15; three blind council seats reached the same
  split independently).
- `grim status` on files with no record: verify against the pinned content
  when that content is already in the local store and report the true state;
  otherwise report a new additive state for "files present, not verified"
  (working name `untracked`). It never reports `missing` for files that exist
  and never fetches for this (owner, 2026-11-15).
- Cold store: when the old pin's content is not local and the run is online,
  `grim update` fetches it once to prove ownership. Offline it skips adoption
  and warns that the unproven files were left in place (owner, 2026-11-15).

- The ADR covers ownership without state only (adoption, status, cold store)
  and records that both checkout models are supported. Issue 257 follows as
  its own plan (owner, 2026-11-15).
- Documentation explains both models in one place with when to use each:
  commit outputs when agents must work without running grim (cloud sessions,
  fresh worktrees), ignore them when every contributor runs `grim install`.
  The guides link there (owner, 2026-11-15).

## Threads

- **Closed: where adoption runs and what status reports.** See Decisions.
  Premortem's worry that an extra hand-placed file inside a skill directory
  could be adopted and later deleted does not hold: `footprint_hash_for_kind`
  in `src/install/content_hash.rs` walks every file under the destination.
- **Observation that frames the fix.** In the committed model the checkout's
  disk matches its lock, because whoever moves the lock commits the resulting
  tree, deletions included. The one place disk and the new lock disagree is
  inside the cloner's own `grim update`, and there the previous lock still
  describes the disk. Adopting against the previous lock before the relock
  gives every old-version name a record, so the existing install pass and
  `prune_orphans` remove what the new version dropped.
- **Closed: cold store.** See Decisions.
- **Closed: published guidance.** Two guide pages tell teams to ignore rendered
  outputs while this repository and a third page commit them. See Decisions.

## Files and interfaces this touches

- `src/command/update.rs`: the previous lock is in scope before
  `lock_io::save`; the adoption pass runs there, ahead of
  `install_all_before_reap` and `prune_orphans_off_runtime`.
- `src/command/install.rs`, `src/command/uninstall.rs`, `src/command/remove.rs`:
  the same adoption pass against the lock as found.
- `src/install/installer.rs`: the untracked-destination gate
  (`RefusedUntracked`, the adopt-if-identical branch) is the logic to reuse.
- `src/install/content_hash.rs` (`footprint_hash_for_kind`),
  `src/install/expected_outputs.rs` (`destination`), `src/install/prune.rs`
  (`prune_orphans`, unchanged in principle).
- `src/command/status.rs` (`derive_state`) and `src/api/artifact_status.rs`
  (`ArtifactStatus` gains one appended literal).
- `src/api/update_report.rs`: the row for a dropped member
  that was left on disk.
- Docs: `docs/src/content/docs/guides/shared-skills.md`,
  `docs/src/content/docs/guides/team-ci.md`,
  `docs/src/content/docs/guides/use-a-marketplace.md`,
  `docs/src/content/docs/json-interface.md`,
  `docs/src/content/docs/upgrading.md`, plus the first-party skills under
  `catalog/` that describe `status` and `update`.
- Consumer: `grimoire-vscode` reads `grim status --format json` and must
  tolerate the new literal.

## Research

- `.agents/research/research_output_ownership_without_state.md`
- `.agents/research/research_generated_files_ownership_prior_art.md`

## Related

- [grimoire-rs/grimoire#257](https://github.com/grimoire-rs/grimoire/issues/257)
  "add to gitignore": "flag to enable automatic add of rendered artifacts to
  gitignore" (open, 2026-10-29, no comments).
- `.agents/adr/adr_hooks_support.md`: grim must never rely on a gitignore,
  because `.grimoire/.gitignore` exists only after grim has run; hook
  approvals, dispatch table and launcher are machine-local, and project-scope
  hook registration lands only on a client-gitignored surface.
- `.agents/adr/adr_install_state_portability.md`: project install state moved
  to `.grimoire/state.json`, gitignored by a self-managed `.grimoire/.gitignore`.
- `.agents/plans/bugfix_plan_reaper_ownership.md`: earlier reaper ownership work.
- Code read on 2026-11-15, nothing run:
  - `src/command/status.rs` (`derive_state`): no record means `missing`, disk
    is never inspected.
  - `src/command/update.rs`: the lock is saved before the install pass; the
    old lock is read as `previous` but not used to compute old outputs.
  - `src/install/installer.rs` (untracked-destination gate): no record and a
    differing footprint refuses with exit 65 unless `--force`; an identical
    footprint is adopted.
  - `src/install/prune.rs` (`prune_orphans`): walks state records only.
  - `test/tests/test_clobber_guard.py`: adoption and refusal at the same
    version; no test for "no state, outputs on disk, new version drops files".

## Open questions

- [NEEDS CLARIFICATION: can the adopt-if-identical branch run against a lock
  other than the one being installed without a large change to `install_one`?]
  Recommended: a separate pre-pass that only renders, hashes and writes
  records, sharing the hash and destination helpers, because the install pass
  then needs no change.
- [NEEDS CLARIFICATION: an MCP entry adopted by value is kept on removal today
  (the `adopted` flag: "the inverse of writing nothing is removing nothing").
  Does an entry adopted from a clone get pruned when the new version drops
  it?] Recommended: keep the shipped rule, leave the entry and name it in a
  warning, because value equality in a shared file cannot tell grim's write
  from the user's.
- [NEEDS CLARIFICATION: scope of adoption under `grim update <names>`.]
  Recommended: adopt only inside the run's closure over the previous lock,
  matching where a named run already installs and prunes.
- [NEEDS CLARIFICATION: the old pin's content is gone from the registry.]
  Recommended: treat it like the offline case, leave and warn.
- [NEEDS CLARIFICATION: a checkout whose committed outputs do not match its
  committed lock (someone moved the lock and did not commit the outputs).]
  Recommended: no adoption for the mismatching artifacts and the existing
  refusal, since nothing proves ownership.
- [NEEDS CLARIFICATION: the name and plain-text rendering of the new status
  literal, and whether `outputs_pending` still lists an unverified
  destination.] Recommended: `untracked`, with `outputs_pending` unchanged.
- [NEEDS CLARIFICATION: does the same adoption apply at global scope, where
  `$GRIM_HOME/state/global.json` can be lost while `~/.claude` outputs
  remain?] Recommended: yes, one rule for both scopes.

## Verification

- Acceptance tests under `test/tests/`, beside `test_clobber_guard.py`, each
  starting from an installed project whose `.grimoire/` directory is then
  deleted:
  - `grim update` to a version that drops a file inside a skill removes it.
  - `grim update` to a bundle version that drops a member removes that
    member's outputs, and a renamed rule's old file is removed.
  - A hand-edited output, and a skill directory holding an extra file, are not
    adopted: the still-shipped artifact is refused with exit 65 and left, and
    a dropped one is left and named in a warning.
  - Offline with a cold store: adoption is skipped, the leftovers remain, the
    warning names them, and no row says `Removed` for them.
  - Online with a cold store: the old pin is fetched and the stale files go.
  - `grim status` after the deletion reports the true state when the content
    is local and the new literal when it is not, creates no `.grimoire/`
    directory, and makes no request for this.
  - `grim install` at the same version still adopts and reports `unchanged`.
- A unit test that the update report row for a dropped member with no record
  is not `Removed` unless files were deleted.
- `task verify` passes; `task catalog:verify` and `task docs:check` pass after
  the docs and skill updates.
