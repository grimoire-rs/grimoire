# Plan: Local-Path Bundles + TUI Local Group

## Status

- **Plan:** plan_local_bundles_tui_group
- **Active phase:** 3 — Review-Fix Loop + docs + gate
- **Step:** finalized
- **Last update:** 2026-07-12 (/finalize: keep-as-is — branch is 16 clean Conventional Commits already based on main, fast-forwards directly; fold/squash of the 3 review-fix commits declined as infeasible — they structurally bracket the bundles feat, any reorder to make them adjacent conflicts. task --force verify green on HEAD 9861454. NOT pushed.)

---

## Review-Fix Round (post /swarm-review, 2026-07-11)

High-tier `/swarm-review` (full branch vs `main`, 6 Claude reviewers + Codex
terra) returned **Request Changes**. Fix-decisions for this `/swarm-execute`:

- **B1 — local-bundle removal corrupts the lock (CONFIRMED; the "honest
  staleness" claim below was false).** `legacy_drop_from_lock`
  (`remove.rs:262-274`) skips `evict_bundle_members` for a path bundle (no
  `(repo,tag)`), drops the snapshot, **and restamps `declaration_hash` fresh** →
  the lock reads current while retaining orphan members only that bundle
  provided → `grim install` re-materializes the removed bundle. **Fix:** evict a
  path bundle's member lock entries on removal (key eviction on the bundle
  *binding name* carried in member provenance, not `(repo,tag)`); a member
  another source still provides survives. The lock must never list members no
  declaration provides under a fresh hash. Member *files* on disk may remain
  (out of scope). Rewrite `test_remove_local_bundle_drops_declaration_but_keeps_member_files`
  (it encodes the bug) → assert removed-bundle members are gone from the lock AND
  a following `grim install` does not resurrect them.
- **B2 — dev-install name collision undeclares a real declaration (data loss).**
  `uninstall.rs:126-137` undeclares unconditionally. **Fix (root cause):** reject
  a `grim install <path>` dev-install whose intrinsic `(kind,name)` collides with
  a declared binding (exit 64, guidance), keeping dev records disjoint from
  declared bindings so uninstall's undeclare can never drop a declaration the dev
  install didn't own. Regression: colliding dev-install rejected; uninstalling a
  pure dev record leaves unrelated config intact.
- **B3 — `grim status` shows `Source=direct` for local bundles.** The bundle loop
  hardcodes `"direct"`. **Fix:** mirror the skill/rule/agent loop — `path: <path>`
  when `DeclaredSource::Path`.
- **B4 — path-source file disclosure (posture: DOCUMENT, user decision
  2026-07-11).** Keep ADR sub-decision 3 trust model (absolute + relative
  allowed). Extend `warn_absolute_path_sources` to also warn on out-of-tree
  **relative** escapes; reframe the message portability→security; add a
  trust-model note to `docs/src/stability.md`. **No new error paths.**

Also fixing: W1 (`spawn_blocking` at the 7 unwrapped `pack_local_artifact` async
sites), W2 (stale STUB/NO-OP comments), W3 (document the project-wide
`effective_set` degrade scope), W4 (test gaps: `resolve_lock_partial` path guard,
`dev_install --kind`, `add_path_source` branches, `perform_local_dev`), W5 (align
`validate_*`/`collect_files` symlink policy), W6c/d (stale `pinned` doc comment,
dead-code attr). Deferred: W6a/b refactor (tui/app.rs split, XOR dedup — Two Hats,
separate pass), status member-drift→Outdated, add-no-kind guidance, resolve
concurrency, path-entry cap.

---

## Overview

**Status:** Approved (scope) — awaiting /swarm-execute
**Author:** Architect (/architect)
**Date:** 2026-07-11
**Related ADR:** [`adr_local_path_sources.md`](../adr/adr_local_path_sources.md) sub-decisions 5 (local bundles) + 8 (TUI Local group)
**Branch:** `feat/local-path-sources`

Closes the two deferred items from the max-tier `/swarm-review` of the
local-path-sources feature. Both were promised v1 in the ADR; the review
found local bundles **half-built** (config accepts, resolver rejects) and
the TUI Local group **absent**. User decision (2026-07-11): implement both
now rather than amend the ADR to defer.

## Objective

1. **Local-bundle dependencies** — a `grimoire.toml` `[bundles]` value may
   be a local path to a bundle TOML whose members are **registry** refs;
   `lock`/`update`/`status`/`install` treat it like a registry bundle,
   pinned by the SHA-256 of its canonical JSON members layer.
2. **TUI Local root group** — path-declared artifacts and dev-install
   records appear under a "Local" root group in the TUI, with update and
   delete actions.

## Scope

### In Scope

- Local-bundle resolution end to end (config → resolve → lock wire → install
  → status).
- `LockedBundle` wire re-representation to hold a bundle with no registry
  identity (path + hash).
- Local-bundle removal/eviction: **path-bundle member eviction on removal**
  (B1 fix — supersedes the earlier "graceful degrade / honest staleness" claim,
  which the review proved false: the lock was restamped fresh while retaining
  orphan members). On removing a path bundle the lock drops the members only that
  bundle provided; a member another source still provides survives; the lock
  never reads fresh with orphan members. Member files on disk may remain.
- TUI Local group: synthesized rows for path declarations + dev records,
  root grouping, update/install + delete actions.

### Out of Scope (remain deferred follow-ups)

- **Relative members in a local bundle** — rejected with error 65 (ADR
  sub-decision 5: no registry identity to anchor against).
- **Path members** inside a local bundle and **`[mcp]` path values** —
  already deferred in ADR sub-decision 5; unchanged.
- `--watch` change detection (ADR sub-decision 9).
- mtime-keyed repack caching.

## Ground Truth (from two exploration passes, 2026-07-11)

**Reused free (already `pub`, no new code):** `BundleManifest` +
`to_layer_bytes()` (`src/oci/bundle.rs:53-78`), `read_bundle_members(path)`
(`src/command/build.rs:240`), `BundleSource::from_toml_str` +
`parse_member_map` (`src/config/project_config.rs:384-405,548`),
`PathSource::resolve` (`src/config/path_source.rs:73`), `Sha256::hash`, and
**the entire member fetch/install/status path** — a local bundle's members
are ordinary registry work items once expanded.

**Config gate already exists:** `PathValues::{Allowed,Rejected}`
(`src/config/project_config.rs:462-466`); bundles are wired `Allowed`
(`:200`), mcp `Rejected` (`:201`). Correct for implement — leave `Allowed`.

**TUI grouping seam already exists and is free:** `display_split`
(`src/tui/tree.rs:592-597`) roots a row under `Some(source)` verbatim; only
new work is *sourcing* the rows (TUI has never synthesized a row from
anything but `load_catalog`).

## Technical Approach

### The one real blocker — `LockedBundle` wire

`LockedBundle` (`src/lock/locked_bundle.rs:24-37`) is a **direct-serde
`deny_unknown_fields`** struct with a **mandatory** `pinned:
PinnedIdentifier` — it cannot represent a bundle with no registry identity.
This is the crux and the only compat-risky change.

**Chosen representation:** mirror the established `LockedSource` /
`RawLockedArtifact` XOR pattern (`src/lock/locked_source.rs`,
`src/lock/locked_artifact.rs`). Introduce a source discriminant on the
bundle:

```
LockedBundle {
    name: String,
    members: Vec<BundleMember>,
    source: <discriminant>,           // in-memory
}

// in-memory discriminant (mirrors LockedSource)
Registry { repo, tag, pinned: PinnedIdentifier }   // today's fields
Path     { path: PathSource, hash: Digest }        // new

// wire (RawLockedBundle, #[serde(deny_unknown_fields)], TryFrom):
//   Registry arm → { name, repo, tag, pinned, members }   ← byte-identical to today
//   Path arm     → { name, path, hash, members }
//   XOR: (repo+tag+pinned) XOR (path+hash), validated in TryFrom
```

**Non-negotiable compat contract:** a registry-only lock serializes to the
**exact same bytes** as today (frozen declaration-hash corpus + lock
byte-identity tests stay green). The Path arm reuses
`validate_path_hash_algorithm` (`locked_source.rs:29`) so a non-SHA-256
bundle hash is rejected at parse — same as artifact path sources (F7).

### Resolve — local branch in `expand_bundles`

Replace the exit-65 rejection (`src/resolve/resolver.rs:388-396`) with a
local branch when `bundle_source.identifier()` is `None`:

1. Resolve the `PathSource` against the config-dir anchor
   (`PathSource::resolve(anchor)` — same call as `resolver.rs:100`).
2. `read_bundle_members(path)` → `(name, members, metadata)`. **`read_bundle_members`
   does blocking `std::fs` I/O and `expand_local_bundle` is `async` — wrap the
   call in `tokio::task::spawn_blocking`, mirroring `resolve_path_entries`
   (`resolver.rs:100-106`). Blocking I/O in async is block-tier (quality-rust).**
3. **Reject any `MemberRef::Relative` member** → `BundleInvalid` (65). A
   local bundle has no registry directory to anchor a relative member
   against (ADR sub-decision 5). Absolute members pass through unchanged
   (`MemberRef::parse(id)` is already absolute-safe — `member_ref.rs:101`).
4. `BundleManifest::new(members).to_layer_bytes()` → `Sha256::hash` = the
   pin. On re-resolve (`update`/re-`lock`) a changed source yields a new hash
   → the lock **rolls forward** (like a floating tag), not a 65. Source drift
   surfaces as `Outdated` at `status` (mirror artifact F6). **There is no
   install-time bundle re-pack** — a bundle materializes its members (each
   registry-digest-pinned), not the bundle layer, so no 65 install-integrity
   gate applies to the bundle itself. **Exit codes (corrected at review):**
   the **65** (DataError) cases are resolve-time rejects — relative member,
   missing file. Parse-time lock-file rejects (non-SHA-256 hash, XOR
   violation) fire inside `TryFrom<RawLockedBundle>` and surface as
   `LockErrorKind::TomlParse` → **78** (ConfigError), consistent with the
   `LockedArtifact` path-arm precedent (a malformed lock is a config error,
   not a data error).
5. Build the `LockedBundle::Path { path, hash }` snapshot.

Members then ride the identical `ExpandedMember` → `build_work` →
`resolve_work` path — unchanged. **Anchor plumbing:** thread `anchor:
&Path` into `expand_bundles` (available at `resolver.rs:60,194`).

### Offline effective-set match

`effective_set::snapshot_matches` (`src/lock/effective_set.rs:117-129`)
keys on `repo`+`tag`+`Identifier`; it already returns `None` (graceful
degrade) for a path bundle (`effective_set.rs:70-75`) — the caller
early-returns before the snapshot arm is ever reached for a path-declared
bundle. **Decision (revised at Specify):** keep the graceful degrade. The
`snapshot_matches` Path arm returns **`false`** (a path snapshot never
matches a registry-declared id — cross-source is always false), which is
both correct and unreachable-in-practice; it needs **no** new signature.
Full path-keyed eviction provenance for local bundles is a **deferred
follow-up** — plumbing a declared `PathSource`/`Digest` through
`snapshot_matches` + un-early-returning the caller is out of scope for v1.
Effect: TUI-deleting a local *bundle* declaration removes it from
config/lock but leaves member files (honest staleness); path *skills/rules/
agents* and dev records delete fully (they are not effective-set-mutation
cases). Local bundles are niche; TUI-deleting one is niche-of-niche.

### TUI Local group

- **Row sourcing (net-new):** `local_rows(config, lock, install_state)`
  builds `TuiRow`s for (a) declared path artifacts (installed or not) and
  (b) dev records (`InstallRecord.dev == true`). Set `source =
  Some("Local")` so `display_split` roots them under the "Local" group
  (free). Wire into `reload_into` (`src/tui/app.rs:988-992`). Inputs already
  loaded by `load_scope_for_badges` (`app.rs:~1520`) — no new I/O.
- **Actions (path/dev branches in two registry-coupled seams):**
  - `perform` (`app.rs:1915`, install/update): bypass the empty-`registry`
    guard (`:1928`) for Local rows; route a **declared** path row to a
    declared install and a **dev** record to `install_and_persist` with
    `InstallIntent::Dev`.
  - `perform_uninstall` (`app.rs:1771`): route a declared path row to the
    undeclare seam (`remove.rs`) + file removal; a dev record to the
    record-drop path. Both underlying seams already exist and are tested on
    this branch.

### Key Decisions

| Decision | Rationale |
|----------|-----------|
| `LockedBundle` → source discriminant (Registry XOR Path), mirroring `LockedSource`/`RawLockedArtifact` | Established codebase pattern (ADR Option 1); exhaustive matches force every consumer to handle the path case; registry wire stays byte-identical |
| Reject relative members in a local bundle (65) | ADR sub-decision 5 — no registry directory to late-bind against |
| `snapshot_matches` Path arm = `false` (graceful degrade retained); full path-keyed eviction deferred | Caller early-returns `None` for path bundles; reaching a rich path match needs signature surgery for a niche-of-niche case (TUI-delete of a local bundle). Cross-source-never-match is correct + zero-risk |
| TUI `i`/update on a **declared** path row = declared install; on a **dev** record = `InstallIntent::Dev` | A config-declared path dep is not a dev install; preserves the declared/dev distinction from the F1 refactor |
| Reuse `read_bundle_members` (build.rs) rather than a new `pack_local_bundle` | It already reads+parses+emits members; the `unreachable!()` in `local_pack.rs:85` stays correct (bundles pack on the resolver path) |

## Implementation Steps

> Contract-First TDD: Stub → Verify → Specify → Implement → Review.

### Phase 1 — Local-bundle resolution

**1.1 Stub — `LockedBundle` source discriminant**
- Files: `src/lock/locked_bundle.rs`, `src/lock/grimoire_lock.rs`
- Introduce the in-memory discriminant + `RawLockedBundle` + `TryFrom` XOR
  shell (`unimplemented!()` bodies). Accessors: `pinned()`, `path()`,
  `content_digest()`, `repo()`/`tag()` where callers need them.

**1.2 Stub — resolve local branch**
- Files: `src/resolve/resolver.rs`
- `expand_bundles` local branch + a `require_absolute_members` helper shell;
  thread `anchor: &Path`.

**1.3 Stub — effective-set path match**
- Files: `src/lock/effective_set.rs`
- `snapshot_matches` path-keyed arm shell.

**Phase 2 — Architecture review** (`worker-reviewer`, spec-compliance,
post-stub): verify the `LockedBundle` discriminant preserves the registry
wire shape; verify every `LockedBundle` reader
(`effective_set.rs:117-129`, `remove.rs:218,266`, `add.rs:448-454`,
`schema.rs:268`) is accounted for. **Gate before implementing** — this is
the compat-risk surface.

**Phase 3 — Specify (tests first)**
- Unit (`src/lock/locked_bundle.rs`): registry arm round-trips
  byte-identical; path arm XOR wire; non-SHA-256 bundle hash rejected;
  `deny_unknown_fields` still holds.
- Unit (`src/resolve/resolver.rs`): local bundle with absolute members
  resolves; **relative member → 65**; hash mismatch → 65; repack-twice-same
  -hash.
- Unit (`src/lock/effective_set.rs`): path-keyed snapshot match hits +
  misses.
- Acceptance (`test/tests/`): declare a local bundle (`[bundles] x =
  "./bundles/x.toml"`) with registry members → lock/install/status; edit a
  member set → `update` rolls forward; `GRIM_OFFLINE=1` install of a locked
  local bundle (members must be cache-resident); registry-only lock stays
  byte-identical (**frozen corpus untouched**).

**Phase 4 — Implement** 1.1–1.3 until green. Gate: `task rust:verify`.

### Phase 2 (plan) — TUI Local group

**2.1 Stub — `local_rows` + action branches**
- Files: `src/tui/app.rs` (`local_rows`, `perform`/`perform_uninstall`
  branches), `src/tui/tree.rs` (label the "Local" root), `src/tui/render.rs`
  / `src/tui/detail.rs` (path/hash cells instead of tag/version).

**2.2 Specify (tests first)**
- Unit (`src/tui/`): a config with a path skill + a dev record produces
  Local-group rows; a registry row is unaffected; badge match still keys on
  `source.pinned()` (path rows never contaminate catalog badges).
- Acceptance (`test/tests/`): TUI smoke — path/dev artifacts render under
  Local; update + delete actions on a Local row route to the dev/declared
  seams and match `grim status`/`uninstall` behavior.

**2.3 Implement** until green. Gate: `task rust:verify`.

### Phase 3 (plan) — Docs + catalog + final gate

- Files: `docs/src/commands.md`, `docs/src/stability.md` (local-bundle wire
  note), catalog `grim-usage`/`grim-authoring` skills (drift review per
  `catalog/README.md`).
- Full `task verify` + `task catalog:verify`.

## Files to Modify

| File | Action | Description |
|------|--------|-------------|
| `src/lock/locked_bundle.rs` | Modify | Source discriminant (Registry XOR Path) + `RawLockedBundle` XOR wire |
| `src/lock/grimoire_lock.rs` | Modify | Serialize/deserialize via `RawLockedBundle`; keep registry bytes identical |
| `src/resolve/resolver.rs` | Modify | `expand_bundles` local branch; reject relative members; anchor plumbing |
| `src/lock/effective_set.rs` | Modify | Path-keyed `snapshot_matches` |
| `src/command/remove.rs` | Modify | Local-bundle removal keys on path/hash |
| `src/command/add.rs` | Modify | Ensure a just-added local bundle's member projection works |
| `src/command/schema.rs` | Modify | Regenerated `$defs.LockedBundle`; assertion update |
| `src/tui/app.rs` | Modify | `local_rows` sourcing + `perform`/`perform_uninstall` path/dev branches |
| `src/tui/tree.rs` | Modify | "Local" root label |
| `src/tui/render.rs`, `src/tui/detail.rs` | Modify | Path/hash cells for Local rows |
| `docs/src/commands.md`, `docs/src/stability.md` | Modify | Local-bundle deps documented |
| `test/tests/test_path_deps.py` (+ TUI test) | Modify | Local-bundle + TUI Local acceptance |

## Testing Strategy

### Unit (from contracts)

| Component | Behavior | Edge cases |
|-----------|----------|------------|
| `LockedBundle` wire | Registry arm byte-identical; Path arm XOR round-trips | non-SHA-256 hash → reject; both field sets present → reject; neither → reject |
| `expand_bundles` local | Absolute-member local bundle resolves + pins by members-layer hash | relative member → 65; hash mismatch → 65; missing file → 65; repack-same-hash |
| `snapshot_matches` path | Matches on path+hash | moved path same hash still matches (path advisory); changed hash misses |
| TUI `local_rows` | Path decl + dev record → Local rows | catalog rows unaffected; path source never flips a catalog badge |

### Acceptance (from UX)

| User action | Expected | Error cases |
|-------------|----------|-------------|
| Declare `[bundles] x = "./b.toml"` (registry members), `grim lock` | Lock has a `Path` bundle entry; `install` materializes members | relative member → exit 65 with clear message |
| `grim update` after editing member list | Lock rolls forward; new members installed, dropped ones pruned | — |
| `GRIM_OFFLINE=1 grim install` of locked local bundle | Succeeds if members cache-resident | member not cached → clean offline failure |
| Registry-only project | Lock bytes unchanged vs pre-change (frozen corpus) | — |
| `grim tui` with a path skill + dev record | Both under "Local" root; update/delete work | — |

## Risks

| Risk | Mitigation |
|------|------------|
| `LockedBundle` wire change breaks registry-lock byte-identity / frozen declaration-hash corpus | Registry arm must serialize byte-identical; add a regression test asserting a registry-only lock is unchanged **before** touching the struct; run frozen corpus each round |
| Every `LockedBundle` reader assumes registry identity (`effective_set`, `remove`, `add`, `schema`) | Phase-2 architecture-review gate enumerates all readers before implement; compiler-driven exhaustive match on the new discriminant |
| Path-keyed snapshot match subtly wrong → local-bundle eviction drops a still-held member | Dedicated unit tests for match hit/miss + an acceptance test evicting one of two bundles sharing a member |
| TUI action branches route a Local row into the registry-only seam | Explicit branch on `source == Some("Local")` before the empty-`registry` guard; acceptance test drives update + delete on a Local row |

## Handoff

- **To `/swarm-execute`**: this plan. Recommend **high** tier + `--codex`
  (one-way-door lock-wire change warrants the cross-model gate) — same
  config as the review-fix cluster. Phase 1 (bundles) first; it carries the
  compat risk. Phase 2 (TUI) can start once the `LockedSource`-shaped
  bundle discriminant lands (local-bundle rows depend on it; skill/rule/
  agent path + dev rows do not).
- **To `/swarm-review`**: after both phases, adversarial re-review with the
  compat corpus as the anchor.

## Progress Log

| Date | Update |
|------|--------|
| 2026-07-11 | Plan created by /architect; scope (implement both) approved by user; two exploration passes captured ground truth |
| 2026-07-11 | `/swarm-execute` Implement pass (Phase 1 — bundles). All 22 Phase-1 unit tests green; `task rust:verify` GREEN (fmt + clippy `-D warnings` + 1637 unit tests); the 5 new acceptance tests collect (need a live registry, run later). Three contract clarifications resolved at implement time, none a signature change: (1) **`LockedBundle` `TryFrom` re-attaches the advisory `tag`.** The registry arm strips the advisory tag from `pinned` on serialize for byte-identity (per the stub doc), but the spec test `registry_arm_round_trips_byte_identical_to_legacy_shape` also asserts `assert_eq!(back, bundle)` with a tag-carrying `pinned` — and `PinnedIdentifier` equality includes the tag. So `TryFrom` restores the tag from the sibling `tag` field onto `pinned` (`clone_with_tag(tag).clone_with_digest(pinned.digest())`), making the in-memory round-trip lossless. This matches the resolver's own construction (`bundle_id.clone_with_digest(digest)` keeps the declared tag). Byte output is unaffected (the `tag` field is separate; `pinned` is re-stripped on the next serialize). For a *digest-only* bundle declaration the reattached "tag" is the short-digest string (odd but in-memory-only, advisory, untested, and byte-stable). (2) **Local-bundle member provenance encoding.** A local bundle's members ride the registry `ExpandedMember`→`merge_bundle_members` path, which needs a `(bundle_repo, bundle_tag)` pair. Chosen: `bundle_repo = path.as_str()`, `bundle_tag = members-layer-hash.to_short_string()`. This keeps a single member's lock provenance on the legacy `bundle`/`bundle_tag` pair (never `repo = `, satisfying acceptance `test_local_bundle_lock_writes_path_hash_entry_and_installs_members`), and `command::add::install_added` reconstructs the same pair from the `LockedBundleSource::Path` snapshot (`b.path().as_str()` + `b.content_digest().to_short_string()`) — a documented lockstep coupling replacing the `unimplemented!()` stub. (3) **No `status.rs`/`update.rs` changes needed.** The existing generic bundle-row (`scope.set.bundles.keys()`) and bundle-member (`is_from_bundle()`) machinery reports local bundles + their members correctly; `grim update` (no names) always full-resolves, so an edited members file rolls the lock forward. **Deferred gap (noted, not implemented):** bundle-*source*-drift → `Outdated` at `status`. Editing a local bundle's members file does not change the declaration hash, so the bundle row still reads `installed` until the next `lock`/`update`. The artifact `path_source_drifted` machinery packs a skill/rule/agent, not a bundle members layer, so there is no *cheap* reuse (a bundle drift check needs read_bundle_members + repack + hash-vs-`lock.bundles`). No acceptance test requires it. Follow-up if desired. |
| 2026-07-11 | `/swarm-execute` Specify pass (qa-engineer, unit + acceptance) flagged two Testing Strategy gaps against the Phase 1.1–1.3 stub signatures — noted here rather than invented into tests: (1) `expand_bundles local`'s "hash mismatch (lock hash ≠ recomputed) → 65" row has no counterpart in `expand_local_bundle`'s given signature (no "previous hash" parameter; its own stub comment is `resolve → read_bundle_members → require_absolute_members → pack → hash → build snapshot`, no comparison step) — most likely this row is either (a) shorthand for the F7 non-SHA-256-hash-at-parse contract already covered at the `locked_bundle.rs` wire layer, or (b) describes an install-time drift check (mirroring `pack_verified_local` in `installer.rs`) not yet stubbed for bundles (`ArtifactKind::Bundle` never reaches the per-artifact install loop — `installer.rs:1059`). Needs architect clarification before Phase 4 implement. (2) `snapshot_matches`'s Path-arm Testing Strategy row ("same path+hash → match; changed hash → no match; moved path same hash → still match") cannot be exercised through the current `snapshot_matches(binding: &str, declared_id: &Identifier, snapshot: &LockedBundle)` signature — there is no parameter carrying the *declared* `PathSource`/`Digest` to compare against the snapshot's, and `effective_set`'s only call site early-returns via `?` on `declared_source.identifier()` (always `None` for a path declaration) before ever reaching `snapshot_matches`. The one contract testable today (a registry `declared_id` never matches a Path snapshot — cross-variant false, mirroring `LockedSource::eq_content`) is covered; the richer path-vs-path matching needs a signature change (e.g. `declared: &DeclaredSource`) or a second function, decided at implement time. |

---

## Review Round 2 — re-review of fix commit `48e2087` (2026-07-11)

High-tier `/swarm-review` of the fix commit alone (`d93ce0a..HEAD`, 6 Claude
reviewers + Codex terra). **Verdict: Request Changes.** The 6 Claude
reviewers all passed the B1–B4/R1/Codex fixes as correct+complete; **Codex
cross-model found two High correctness bugs the panel missed**, both
source-confirmed. Security added one actionable Warn.

### Blockers (fix before land)

- **C1 — legacy direct-artifact removal drops a bundle-provided member under
  a fresh lock** (`src/lock/effective_set.rs:70-81` + `src/command/remove.rs:258-300`).
  `effective_set` returns `None` for the WHOLE declared set when any bundle
  is unresolvable offline; a path bundle is ALWAYS unresolvable offline
  (no identifier), so **any project with one path bundle routes every
  mutation through `legacy_drop_from_lock`**. Its direct arms
  (`Skill/Rule/Agent/Mcp`) do unconditional `retain(|a| a.name != name)`
  with NO bundle-provision check, then restamp `declaration_hash` fresh.
  Removing a directly-declared skill that a **registry bundle also provides**
  therefore drops it from a lock that reads fresh → `install`/`update` never
  restores it until a manual `grim lock`. B1 upgraded only the *Bundle* arm;
  the direct arms stayed lossy.
  *RCA root:* the whole-call-degrade design (`adr_effective_set_mutations`)
  assumed "membership unknowable offline" was rare (pre-cache/hand-edit) and
  let the legacy path be lossy; local-path bundles make it the NORMAL case.
  *Fix:* make the legacy direct-arm bundle-aware — before dropping a direct
  `(kind,name)`, scan `previous.bundles` cached snapshots for a
  still-declared bundle listing it as a member; keep it (re-derive
  provenance) or mark honest-stale on mismatch, matching the non-legacy
  `Origin::Bundles` path. (Alt: per-bundle degrade so a path bundle degrades
  only its own membership — larger change.) Regression: path bundle present
  + direct skill also in a registry bundle → `remove` that skill → it stays
  in the lock / reinstalls.

- **C2 — B2 collision guard is install-time only; reverse order re-opens the
  data-loss** (`src/command/add.rs:383-404`, `src/command/uninstall.rs:126-137`).
  `grim add <path> --no-install --name foo` after `grim install <path>` (dev
  record, same `(kind,name)`) writes the declaration checking ONLY
  `scope.set` (declarations), never install-state dev records. Declaration +
  `dev:true` record then coexist; `grim uninstall <kind> foo` calls
  `undeclare_and_unlock` unconditionally (no `record.dev` branch) → drops the
  real declaration. Same destructive collision B2 fixed, via the opposite
  operation order.
  *RCA root:* the invariant was framed as "reject colliding dev-install"
  (one operation) instead of "the shared `(kind,name)` keyspace must never
  have both a declaration and a dev record populated" (both operations).
  *Fix:* enforce disjointness at BOTH creation paths — `grim add` must
  reject (or migrate) when a same-key `dev:true` record exists; dev-install
  already rejects the reverse. Optionally also branch `uninstall` on
  `record.dev` (defense-in-depth at the consuming end). Regression:
  dev-install → `add --no-install` same name → `uninstall` → declaration
  survives.

### Actionable Warns (fix in same loop)

- **S1 (security) — `resolves_outside_workspace` `Vec::pop()` underflow
  bypass** (`src/command/scope_resolution.rs:181-202`). The empty-base fix
  patched only the first pop-past-zero. For a **relative** workspace base
  (reachable via `grim status --config sub/grimoire.toml` or the MCP
  `workspace`/`config` tool-call params), a crafted
  `../../sub/escape.md` pops past all base components (silent no-op on empty
  vec) then re-spells `sub` via a `Normal` push → false "in-tree" → SECURITY
  warning suppressed. PoC-verified. Warning-only (no new read/write; trust
  model already permits the read), but defeats the audit control this commit
  adds and makes the `stability.md` "resolves outside the workspace root"
  claim inaccurate for that input. *Fix (one line):* in the `ParentDir` arm,
  `if resolved.pop().is_none() { return true; }`. Regression: shallow
  relative base (`Path::new("sub")`) + `../../sub/escape`.
- **T1 (test-coverage) — B2 collision tested only for `Skill`**
  (`test/tests/test_dev_install.py`). The `Rule`/`Agent` arms of the
  install.rs collision check have zero coverage. Add a rule-kind analog of
  `test_dev_install_name_collision_with_declared_binding_is_rejected`.
- **D1 (docs) — `commands.md#add-path` silent on the new relative-escape
  warning** (only documents the absolute-path case). Add a sentence / link
  to `stability.md#limitations-path-source-trust`.
- **D3 (docs) — catalog `grim-usage/references/consume.md` stale on the
  exit-64 dev-install collision reject** (CLAUDE.md catalog drift duty). Port
  the fact from `commands.md#install-dev`.

### Deferred (documented, not blocking)

- **S2** — `parse_artifact_map` (`src/config/project_config.rs`, out of this
  diff) doesn't charset-validate binding *keys*, so a hand-edited mixed-case
  key could slip past B2's exact-match `contains_key`. Requires hand-edited
  config (trusted input); defense-in-depth follow-up: enforce
  `SkillName::parse` on binding keys at config-load.
- **D2 (docs)** — `stability.md` command list undercounts callers (true
  "every project-scope command" claim, incomplete parenthetical); reword to
  "e.g." or list all.
- Symlinked path-source **root**/ancestor (CWE-59 leaf-only, accepted trust
  model), TOCTOU on symlink check-then-read (CWE-367, accepted), sequential
  `spawn_blocking` in `status.rs`/`update.rs` (JoinSet, not introduced here),
  `derive_dev_state` residual blocking-fs (unchanged by diff), tui/resolver
  hand-rolled pack dupes (Two-Hats refactor), R1 stale-snapshot-over-live
  corner (accepted per `adr_effective_set_mutations`), Windows symlink test
  gap (no Windows CI target).

### Progress Log

| Date | Update |
|------|--------|
| 2026-07-11 | `/swarm-review` high re-review of `48e2087` (base `d93ce0a`). 6 Claude reviewers: spec-compliance/quality/performance **Pass**, docs **Gaps** (3 Warn), test-cov **Pass** (1 Warn T1), security **No Block** (1 Warn S1). Codex terra: 2 High (C1 lock-integrity, C2 reverse-order data-loss), both source-confirmed. Verdict **Request Changes** → `awaiting /swarm-execute (review-fix loop)`. |
