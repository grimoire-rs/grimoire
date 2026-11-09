# Research: Lock pin stability — codebase recon

## Metadata

**Date:** 2026-11-03
**Domain:** packaging
**Triggered by:** /hex-discuss lock-pin-stability (.agents/discussions/lock-pin-stability.md)
**Expires:** 2027-05-03

## Research: grim lock pin reuse — codebase recon

All claims were read from source in this worktree (branch `uno`). Nothing was executed; behavioural statements below are derived from code reading plus the existing tests named in finding 14. Paths are relative to the repo root.

### Direct answer (short)

- The serialized `pinned` of a `[[skill|rule|agent|mcp|hook]]` entry is `registry/repo@sha256:…` with the requested tag stripped at write time (`src/lock/grimoire_lock.rs:571`). A directly declared artifact's tag is therefore not in the lock at all. The lock never holds the reference "as the user wrote it"; `grimoire.toml` always stores a fully-qualified identifier (short ids are expanded by `grim add` before the config write, and a registry-less value is a config error), and the `declaration_hash` is the only lock-side record of that request.
- Tags are recorded in four places: `[[bundle]]` `tag` (declared bundle tag), `bundle_tag` / `[[skill.bundles]].tag` provenance, `[[bundle.member]].id`, and `[[<kind>.requires]].id`. These are the identifiers a bundle or a publisher declared, not the user's.
- `declaration_hash` is SHA-256 over a JCS object of `{kind: {name: Display(identifier-or-path)}}` for every declared table, including tags and digests. It is compared by `install` (exit 65), `status`, `hook list`, `resolve_lock_partial` (exit 65), and marketplace `is_stale`. `remove` restamps it.
- Existing pins are reused in exactly one situation: a partial resolve (`resolve_lock_partial` via `roll_forward` with names), where unselected nodes whose requested identifier equals the previous one keep their digest. `grim lock`, `grim update` (no names), `resolve_lock`, bundle adds, and `grim add` of any changed declaration resolve every tag fresh. `grim add` reaches the partial path only when the declaration hash is unchanged, because the config is written first and the stale guard then fires and falls back to a full `resolve_lock` with no `previous`.
- `remove` and `uninstall` never call a resolver; they edit the previous lock offline.

### Key findings

**A. Lock data model**

1. Entry shape. `LockedArtifact { name, kind (not serialized), source: LockedSource, bundles: Vec<BundleProvenance>, requires: Vec<BundleMember> }` (`src/lock/locked_artifact.rs:46-71`). `LockedSource` is `Registry(PinnedIdentifier)` or `Path { path, hash }` (`src/lock/locked_source.rs:37-52`). The same struct serves skill, rule, agent, mcp and hook; the `[[kind]]` table name carries the kind (`src/lock/grimoire_lock.rs:84-102`). Path sources are only skill/rule/agent; mcp and hook reject path values at config parse (`src/resolve/resolver.rs:132-168`, `src/config/declaration.rs:497-506`).
2. `pinned` keeps no requested tag on the wire. `PinnedIdentifier` in memory is a full `Identifier` that may carry a tag plus a digest (`src/oci/pinned_identifier.rs:15-19`). The artifact serializer writes `pinned.strip_advisory()` (`src/lock/grimoire_lock.rs:571`; doc at `:445-447`, `:485-486`). On read nothing re-attaches a tag for artifacts (`src/lock/locked_artifact.rs:204-206` takes `pinned` as parsed). A lock-loaded artifact pin is therefore `registry/repo@digest` only. Golden fixtures show this: `src/lock/testdata/grimoire_lock/registry_kinds.lock`, `path_and_bundles.lock`.
3. `PinnedIdentifier::eq_content` compares registry + repository + digest and ignores tag (`src/oci/pinned_identifier.rs:36-41`). `LockedSource::eq_content` does the same for registry pins and compares `hash` alone for path pins (`src/lock/locked_source.rs:61-67`). `lock_io::save` uses this for `generated_at` preservation (`src/lock/lock_io.rs:57-64`, `:160-209`) and `grim lock` uses it for the `unchanged`/`locked` report (`src/command/lock.rs:109-122`).
4. Declared bundles keep the tag. A registry `[[bundle]]` is `name, repo, tag, pinned, [transitive], [[member]]` (`src/lock/locked_bundle.rs:36-50`, `:80-90`, `:150-175`). `pinned` is written tag-stripped (`:283-285`) and the tag is re-attached from the sibling `tag` field on read (`:227-237`). `tag` is the declared tag, or the digest short form for a digest-only declaration (`src/resolve/resolver.rs:601-610`). A path bundle is `path + hash` (`:40-49`).
5. Bundle members. Each `[[bundle.member]]` is a `BundleMember { kind, name, id }`, where `id` is the member id as the bundle published it (`src/oci/bundle.rs:44-54`). The snapshot is the declared bundle's member list at resolution time (`src/lock/locked_bundle.rs:80-90`).
6. Member provenance. A bundle-contributed artifact carries `bundle` + `bundle_tag` (one contributor) or `[[skill.bundles]] repo/tag` (two or more) (`src/lock/locked_artifact.rs:19-27`, `:157-203`). `repo` + `tag` is a `BundleProvenance` (`:17-24`). A path-sourced entry cannot carry provenance (`:208-211`).
7. `requires` edges. `[[<kind>.requires]]` is `Vec<BundleMember>` "exactly as published (a relative `id` stays relative)", omitted when empty (`src/lock/locked_artifact.rs:66-70`). Edges are re-resolved against the parent's pinned repository on every read (`src/resolve/resolver.rs:398-404`). A path source's edges come from its source declaration and are stored the same way (`src/resolve/resolver.rs:149-165`).
8. Transitive bundles. `LockedBundle.transitive` is serialized `transitive = true` only when no `[bundles]` declaration names the snapshot (`src/lock/locked_bundle.rs:86-90`, `:166-170`). `has_dependency_data` treats a lock with any `requires` or any transitive bundle as dependency-bearing (`src/command/remove.rs:310-312`).
9. User-written reference. Not recorded in the lock. The config parser accepts only fully-qualified values and adds `:latest` to untagged ones (`Identifier::parse(value)` then `id.or_latest()`, `src/config/project_config.rs:983-985`; registry-less input is `ArtifactValueMissingRegistry`, `:986-994`). `grim add <short-id>` expands against the scope's registry set before anything is written, via `declare_registry` (`src/command/add.rs:165-175`), so the short form and the default-registry used never reach the lock or the hash. Schema: `pinned` is described as "Resolved registry/repo pinned to a content digest" (`src/lock/testdata/lock.schema.json`, `LockedArtifact.properties.pinned`); the schema has no field for a requested tag on an artifact. (The published copy `docs/public/schemas/grimoire-lock.schema.json` does not exist at that path in this worktree; only the testdata schema was read.)

**B. `declaration_hash`**

10. Inputs. `hash::declaration_hash(&DesiredSet)` builds a JCS string `{["agents"],["bundles"],["hooks"],["mcp"],"rules","skills"}`, each a `{name: Display(source)}` table, keys sorted (`src/config/hash.rs:50-95`). Sources are the registry `Identifier` Display form (`registry/repo[:tag][@digest]`) or the raw `./…` path string (`:100-127`). Agents, bundles, hooks and mcp keys are emitted only when non-empty (`:50-83`). `DECLARATION_HASH_VERSION = 1` (`:27`), written to `metadata.declaration_hash_version` (`src/resolve/resolver.rs:477`); a lock with another version fails to load (`src/lock/grimoire_lock.rs:183-190`). The hash covers tags and digests, so retagging in the config changes it. It does not cover the lock, the resolved digests, `requires`, or `[options]`. It is computed lazily and cached by `DesiredSet::declaration_hash_cached`, cleared by `invalidate_declaration_hash_cache` (`src/config/declaration.rs:586-593`); `declare` invalidates (`src/command/add.rs:957`).
11. Comparators and mismatch behaviour:
    - `grim install` (`load_fresh_lock`): `LockStale` (exit 65, "run `grim lock` before installing"), `src/command/install.rs:436-442`, `src/command/command_error.rs:24`.
    - `resolve_lock_partial`: `StaleLock` (65), before any I/O (`src/resolve/resolver.rs:520-532`). `roll_forward` with names and a predecessor goes there (`:578-597`).
    - `grim status`: every row becomes `stale` (`src/command/status.rs:153-154`, `:222-223`, `:1610-1612`); `grim hook list` reads the same flag (`src/command/hook/list.rs:161-162`).
    - Marketplace: `is_stale` compares per-plugin hashes (`src/export/resolve.rs:102-107`); a `Members` pick against a stale part is `StaleLock` (`:332-340`).
    - `grim remove` restamps the hash to the post-edit set unless an id-mismatch left the lock stale (`src/command/remove.rs:286-288`); `target_only_drop` deliberately does not restamp (`:314-330`).
    - `grim add` and `update` write a fresh hash through `build_lock` (`src/resolve/resolver.rs:473-478`).

**C. Lock writers**

12. Writers of `grimoire.lock` (all via `lock_io::save`, `src/lock/lock_io.rs:57`):

| Caller | Entry point | Reuse of existing pins |
|---|---|---|
| `grim lock` (`src/command/lock.rs:46-67`) | `roll_forward(set, previous, &[])` → `resolve_graph(.., selected=None)` (`src/resolve/resolver.rs:593-596`) | None. `build_carry` returns before filling `digests` when `selected` is `None` (`:349`); every tag resolves fresh. `previous` is used only for the `requires` offline fallback (`:341-348`) and for `generated_at`/report. |
| `grim update` no names (`src/command/update.rs:109-122`) | same as `lock` | None. |
| `grim update <names>` | `roll_forward` → `resolve_lock_partial` | Unselected nodes keep pins (finding 13). Stale predecessor is 65 (`:520-532`). |
| `grim add` registry/path (`src/command/add.rs:287-333`) | `write_config_and_relock` → `relock_declared` → `relock_entry` | Config is written first, then `previous = lock_io::load(..).ok()` (`:312`). Bundle kind: `resolve_lock` full (`:1363-1364`). Other kinds: `resolve_lock_partial([name])` (`:1391`); on `StaleLock` falls back to `resolve_lock` with no previous (`:1392-1401`); no previous: `resolve_lock` (`:1406`). |
| TUI install/update (`src/tui/app.rs:2846-2860`) | `declare` + `write_config` + `relock_declared` | Same as `add`. |
| TUI local-declared install (`src/tui/app.rs:2995-3003`) | `relock_declared` with an unchanged set | Partial relock of the named entry against a fresh predecessor; others carried. |
| `grim remove` (`src/command/remove.rs:95-105`) | `drop_from_lock` over the loaded lock; no resolver | All remaining pins are kept verbatim; entries are dropped or restamped from the cached lock (`:146-296`). |
| `grim uninstall` (`src/command/uninstall.rs:421`) | `outcome.lock` from the same effective-set code | Same. |
| Marketplace `update`/export (`src/command/update.rs:411-459`, `src/export/resolve.rs:377-386`) | `roll_forward` per plugin | `Whole` and a `Members` pick with no part use `prev = None` and no names (full, `:361-363`); a `Members` pick with a fresh part is a partial; unselected parts are carried verbatim (`:420-`). Writes `marketplace.lock` via `save_marketplace`. |

    `lock`, `update`, `add` and the TUI read the previous lock with `.ok()`, so an unreadable or wrong-version lock silently becomes "no previous" (`src/command/lock.rs:49`, `src/command/update.rs:109`, `src/command/add.rs:312`, `src/tui/app.rs:2856`).
13. Why `grim add` of a new or changed entry is a full resolve. `declare` mutates the cloned set and invalidates the hash cache (`src/command/add.rs:943-958`, `:206-217`); the relock hashes this post-edit set while the previous lock holds the pre-edit hash. They differ for any new or changed entry, so `resolve_lock_partial` returns `StaleLock` before any I/O (`src/resolve/resolver.rs:520-532`), `relock_entry` catches it (`src/command/add.rs:1392-1401`), and calls `resolve_lock(set, ..)` with no `previous`: no carry at all. The partial path with carry is reached only when the set hash equals the lock's, e.g. re-adding the identical reference or `perform_local_declared`. This is code reading, not an executed reproduction. The ADR text says the partial applies to `grim add` (finding 17); the stale-guard fallback is the divergence.

**D. Resolver reuse mechanism**

14. `build_carry(set, previous, selected)` (`src/resolve/resolver.rs:339-427`):
    - Always fills `carry.requires` keyed by `((kind, name), digest)` for each locked registry artifact that has `requires` (`:341-348`). Used only on an offline store miss, never when a registry answers (`src/resolve/graph.rs:476-483`, `:1025-1031`).
    - Returns with empty `digests` when `selected` is `None` (`:349`).
    - For partial: `keep(key, id)` skips any key whose name is in `names` (name only, not kind, `:375-378`), looks up the previous pin of `(kind, name)` (`locked`, `:360-373`), and inserts `carry.digests[id] = previous_digest` only if the previous pin's `registry_repository()` equals `id.registry_repository()` (`:379-383`). The comparison is repository only; tag and digest are not compared. For declared roots the tag identity is established by the already-passed hash guard (the id comes from `set`; the previous pin has no tag to compare). For edges the id comes from the previous lock's own `requires`/member ids, so the key is the previous request.
    - Declared roots (skill/rule/agent/mcp/hook registry sources) go first (`:386-401`); then every previous `requires` edge not onto a declared key, resolved against the parent's pinned identifier via `MemberRef::resolve` (`:402-413`); then every previous `[[bundle]].member` with an absolute id not onto a declared key (`:414-423`). Relative member ids in bundle snapshots are skipped (`MemberRef::Absolute` only).
    - Declared bundles are never carried and are re-fetched (doc `:331-338`; the roots loop omits `set.bundles`, and `declared()` excludes bundle keys from edge carry).
    - Path sources: `build_roots` keeps the previous path pin and edges untouched when the entry is unselected, a previous entry exists, and its path equals the declared path (`:149-159`); else re-packs. Path sources are not in `carry.digests`.
15. `Carry` and lookup (`src/resolve/graph.rs:476-483`): `digests: BTreeMap<Identifier, Digest>` keyed by the full requested `Identifier` (registry, repo, tag, digest all part of the key). `expand_graph` seeds `Graph.digests` from it (`:1005-1008`). `resolve_digests` skips any pending id already in `digests` (`:607`); `fold` does the same for edge ids (`:761`). A different tag therefore has a different key and resolves fresh (unit test `updating_the_parent_keeps_the_unselected_childs_pin_for_its_old_request`, `src/resolve/resolver.rs:1279-1297`, and `a_changed_parent_edge_resolves_the_new_child_and_keeps_the_unchanged_one`, `src/resolve/graph.rs:2321-2358`).
16. Fresh resolve and offline. `retry_chain` calls `access.resolve_digest(id)` (`src/resolve/resolver.rs:619-640`). A digest-bearing id resolves to itself without I/O (`src/oci/access/cached_access.rs:65-68`, `registry_client.rs:372-376`). Online, `CachedAccess` never reads the tag cache: it resolves from the registry and writes the tag cache on `Operation::Resolve` (`cached_access.rs:88-102`). Offline, it returns the tag-cache digest or `OfflineMiss` (`:70-80`). So offline pin reuse comes from `$GRIM_HOME`'s tag cache (last resolved digest per tag), not from the lock. A manifest/requires-layer store miss offline falls back to the previous lock's `requires` at that digest (`graph.rs:445-470`, `:1029-1031`); bundles have no such fallback.

**E. ADR text vs code**

17. `.agents/adr/adr_artifact_dependencies.md`, Decision 3 "Partial resolve (`grim update <names>`, `grim add`)" (`:585-598`): selected nodes and nodes absent from the previous lock resolve fresh; an unselected child keeps its pin only when its winning requested identifier is unchanged; unreachable nodes drop; declared bundles are re-fetched. Code match: `keep` skips selected names and keys with no previous pin (`resolver.rs:375-382`); carry is keyed by the requested identifier (finding 15); unreachable nodes are absent because the lock is built from the graph (`build_lock`, `:429-`); declared bundles are not carried (finding 14). Divergences: (a) `grim add` is described as partial, but a changed declaration always trips the stale guard and falls back to a full resolve (finding 13), which the ADR does not mention; (b) the stated identifier rule is "the direct entry's ref, or the agreeing edge id resolved against its parent's current repository", while `keep` compares only repository and relies on the hash guard (roots) or the previous lock's own ids (edges); (c) `names` match by name across kinds.
18. Decision 4 (`:600-`, lock shape) matches the serializer: `[[<kind>.requires]]` with ids as published, `[[bundle]] transitive = true`, omitted when unused (finding 7, 8). Offline carry of `requires` is unverified and refused at install by the set-compare, `StaleDependencies`, "run grim lock" (`src/install/install_error.rs:156`). The ADR states "the lock's recorded `requires` is never a trusted cache" and "online, it never influences resolve"; consistent with `graph.rs:476-483`. The ADR does not address pin reuse for `grim lock`/`update` without names; it describes "a steady-state re-lock" as one tag resolve per artifact.

**F. Messages that say "run `grim lock`"**

19. State that triggers each:
    - `no grimoire.lock found at {path}; run grim lock first`: lock file absent (`src/command/command_error.rs:19`), raised by `install` (`src/command/install.rs:424-432`) and `why` (`src/command/why.rs:239`); exit 79.
    - `grimoire.lock is stale (declaration_hash … does not match current …); run grim lock before installing`: hash mismatch at install (`command_error.rs:24`, `install.rs:436-442`); exit 65. Docs: `docs/src/content/docs/commands.md:547`, `:1048`.
    - `status` `stale` row: hash mismatch; remedy `grim lock` (`src/api/artifact_status.rs:27-28`, `commands.md:934`).
    - `the lock's dependencies do not match the pinned artifact; run grim lock`: install set-compare differs from the pinned manifest's requires layer (`src/install/install_error.rs:156`).
    - `local source '{name}' changed since the lock was written …; run grim update {name} or grim lock`: a path source's packed hash differs from the locked `hash` (`install_error.rs:144`).
    - `why`: "dependency graph cannot be computed offline" and "has a cycle" (`src/command/why.rs:251`, `:259`).
    - `remove`: "now required by … at a different version; the lock is marked stale — run grim lock to pin it" (`src/command/remove.rs:221`), "now provided by bundle … at a different version … pin the bundle's version" (`:228`), "provided by bundles that disagree" (`:246`), "kept N artifacts whose dependency graph could not be computed offline; run grim lock" (`:331`). The first three leave the declaration hash un-restamped (`:285-288`).
    - `grim add <local bundle>`: "add the entry and run grim lock" (`src/command/add.rs:1097`).
    - Marketplace: "the lock of project … is stale; run `grim lock` there" (`src/export/stage.rs:532`), "{drift}; run `grim lock` in …" (`:1176`).
    - Docs say "`grim add` already locks what it declares" (`commands.md:488-489`).

**G. Tests**

20. Existing coverage:
    - Unit, `src/resolve/resolver.rs`: `partial_stale_hash_gate_fires_before_any_access` (`:1126`), `partial_carries_forward_unselected_entries` (`:1161`; only a scripted resolve of "a" and an assertion that "b" is carried), `updating_the_parent_keeps_the_unselected_childs_pin_for_its_old_request` and siblings (`:1279-1313`), `c034_no_names_full_resolves_even_against_a_stale_predecessor` (`:2014`), `c034_names_with_a_stale_predecessor_refuse_with_stale_lock_65` (`:2066`).
    - Unit, `src/resolve/graph.rs`: carry tests (`:2230-2358`) — `lock_requires_never_trusted_cache`, offline carry-forward tests, `a_changed_parent_edge_resolves_the_new_child_and_keeps_the_unchanged_one`.
    - Acceptance: `test_update.py::test_update_rewrites_lock_and_rematerializes` (`:39`, a retagged floating tag moves on `update`), `::test_update_named_only_touches_that_artifact` (`:71`, only `a` moves; b's tag did not move, so this shows no stale-pin contrast), `::test_partial_update_with_stale_lock_exits_65` (`:178`); `test_lock.py::test_relock_is_byte_identical_and_preserves_generated_at` (`:43`, no tag moved); `test_add_remove.py::test_add_repins_declared_artifact_to_new_tag` (`:150`, deliberate re-pin through a different tag), `::test_add_same_name_same_reference_is_idempotent` (`:201`, tag unmoved); `test_dependencies.py` (`:93`, a retagged transitive node moves on `grim lock`, asserted as intended behaviour); `test_bundles.py` (`:385-415`, a `--pin` bundle member stays frozen across a retag); `test_fix_add_update_wedge.py` (`:188`, retag then `update`).
21. Gaps (no test found):
    - No test retags a floating tag between two lock writes and then runs `grim add <other>` or the TUI install and asserts the unrelated entry's pin is kept or moved. `grim lock` re-pin-on-retag is asserted as correct only for a transitive node (`test_dependencies.py:93-96`), not for a direct entry.
    - No test of `relock_entry` / `relock_declared` themselves (grep: no `#[cfg(test)]` references in `src/command/add.rs` to `relock_*` beyond the definitions) — the stale-guard fallback to a full `resolve_lock` is untested.
    - No test combines the carry (`Some(names)`) path with a declared bundle whose members carry floating ids across a retag.
    - `partial_carries_forward_unselected_entries` does not retag "b" in its mock, so it cannot distinguish "kept" from "resolved to the same digest".
    - No test that the `.ok()` swallow of a corrupt/wrong-version previous lock changes the outcome of `lock`/`add`.
    - Offline: no test of pin reuse via the tag cache versus the lock for `grim lock --offline` with a moved tag.

### negative:

- The lock does not store the user-typed reference or the default registry used; no field exists for it (`RawLockedArtifact` has `name, pinned, path, hash, bundle, bundle_tag, bundles, requires, plugin` only, `src/lock/locked_artifact.rs:85-121`). `docs/public/schemas/grimoire-lock.schema.json` was not found at that path; only `src/lock/testdata/lock.schema.json` was read.
- No resolver entry point called by `grim lock`, `grim update` (no names), or `resolve_lock` consults `previous` to keep a pin; `previous` is read there only for `requires` fallback and `generated_at`.
- `grim remove` and `grim uninstall` call no resolver (finding 12) — "the lock-writing commands decide per entry" does not apply to them.
- `src/lock/` has no code that compares a requested tag against a recorded tag for an artifact; the only tag comparison against the lock is `snapshot_matches` for declared bundles (`src/lock/effective_set.rs:478-494`).
- `grim install` writes no lock (grep of `lock_io::save` shows only lock, update, add, remove, uninstall, TUI, marketplace).
- ADR Decision 3 says nothing on whether `grim lock` should carry (finding 18).
- Not verified by execution: the finding 13 claim that `grim add <new entry>` re-resolves all pins; no binary was built or run.

### leads:

- Adjacent lane (behavioural): build `grim` and reproduce `lock` / `add` / TUI install with a retagged floating tag against a local registry to confirm finding 13 — the code reading predicts every pin moves on an `add` that changes the declaration.
- Adjacent lane (ADR history): `.agents/adr/` and `.agents/plans/` for the original `grim lock` / `grim add` relock decision (`adr_effective_set_mutations.md`, the C-034 contract named in `src/resolve/resolver.rs:2014`) — whether re-resolve-on-lock was an explicit choice.
- Adjacent lane (docs contract): `docs/src/content/docs/commands.md:485-501`, `concepts.md` (bundle/floating-tag sections), `stability.md` — what the published docs promise about `lock` re-resolving, since Principle 9 treats docs as contracts.
- Adjacent lane (catalog): `catalog/` skills `grim-usage`, `grim-authoring` — any teaching that `grim lock` refreshes floating tags.
- Adjacent lane (marketplace): `src/export/resolve.rs` `Whole` picks use `prev = None`, so a marketplace `update <plugin>` never carries; relevant if pin policy must cover marketplace locks.
- Adjacent lane (upstream prior art): Cargo, npm, uv, Go lockfile semantics for "add one dependency keeps other pins" (ecosystem/prior-art researcher).
