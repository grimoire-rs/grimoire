# Research: Dependency version claims — codebase recon

## Metadata

**Date:** 2026-11-12
**Domain:** packaging
**Triggered by:** /hex-architect "Cargo-style version claims for dependencies" (amends `.agents/adr/adr_artifact_dependencies.md` Decision 3)
**Expires:** 2027-05-12

All claims were read from source in this worktree (branch `uno`). Nothing was executed. Paths are relative to the repo root. "Your brief" below means the architect run that asked for this map.

## 1. Module map

**Entry points.**
- `target_lock` (`src/resolve/resolver.rs:154`) is the one relock function. It runs `build_roots`, then `build_carry` (`:1158`), then `expand_graph` (`:184`), then `build_lock` (`:1233`), then `plan_lock` (`:232`).
- `TargetLock` (`resolver.rs:119`) holds `lock`, `dispositions`, `tagless_resolved`, `kept_over_refusal` and `part`.
- It is called from add (`add.rs:371`), update (`update.rs:141`), lock (`lock.rs:67`), the TUI (`tui/app.rs:3549,4083,8998,9406`) and the export stage (`export/resolve.rs:435`). `install` does not resolve (see 4i).

**Graph state.**
- `struct Graph` (`graph.rs:900`) is private. It holds `nodes`, `carry`, `released`, `attested`, `unattested`, `confirmed`, `digests`, `reads`, `transitive` and `bytes`.
- `released` (`:905`) is a set that only grows: "a pin stays released even if a later pass drops the claim that released it".
- `Node` (`:316`) holds `direct: Option<Direct>`, `claims: Vec<Claim>`, `depth`, `queued` and `read`.
- `Claim` (`:253`) is an edge claim: `parent`, `id`, `listed`, `stamped`.
- `Read` (`:273`) holds `id`, `digest`, `published`, `edges`, `layer_bytes` and `from_manifest`.
- `Pending` (`:342`) is an identifier waiting for its digest. `Job` (`:362`) is a node waiting to be read.

**Claim types.**
- `DirectClaim { id, digest }` is at `src/lock/merge_policy.rs:19`.
- `EdgeClaim { parent, requested_id, digest }` is at `merge_policy.rs:28`.
- `Settled { Direct | Agreed | Conflict }` is at `merge_policy.rs:38`.
- `settle()` is at `merge_policy.rs:63`. It is the one pure rule for how a key settles.
- `NodeClaim { Direct | Edge | ListedEdge | StampedEdge }` (`graph.rs:738`) is the shape a claim has when asking whether a carried pin answers it.

**Carry types.**
- `Carry` (`graph.rs:646`) holds `pins`, `requires`, `proven`, `recorded_only`, `walked` and `part`.
- `CarriedPin` (`:701`) holds `pin`, `declared`, `snapshot` and `evidence`.
- `Carry::answer` (`:774`), `carried_pin` (`:800`) and `kept` (`:851`) are the answer logic. They call `pin_rule::{answers_direct, answers_edge, keeps_pin}` (`src/lock/pin_rule.rs:391,402,428`).
- `Refresh { All, Names, Nothing }` is at `src/lock/refresh.rs:14`. `Refresh::selects` is at `:50`.

**Graph methods** (`graph.rs`):

| Method | Line |
|---|---|
| `seed` | 1223 |
| `resolve_digests` (pass one) | 1276 |
| `claims_of` | 1317 |
| `settled` | 1341 |
| `schedule` | 1359 |
| `fold` | 1423 |
| `expand` | 1497 |
| `conflicts` | 1528 |
| `release_conflicts` | 1543 |
| `restart` | 1567 |
| `finish` | 1583 |
| `contributor` | 1598 |
| `conflict_error` | 1611 |
| `settle_winners` | 1709 |
| `into_graph` | 1733 |
| `expand_graph` (public) | 1820 |
| `expand_counting_passes` | 1843 |
| `walk_until_settled` | 1905 |
| `settle_pass` | 1929 |

**Other types.**
- `ExpandedGraph` (`:230`), `GraphNode` (`:101`) and `GraphWarning` (`:146`: `PublisherMismatch`, `HeldBack`).
- `NodeDisposition` and `plan_lock` are at `src/lock/disposition.rs:28,39`.
- Closure helpers `force_nodes`, `reachable_lock`, `closure_nodes` and `project_lock` are at `src/lock/closure.rs:23,65,221,237`.
- Errors: `ResolveErrorKind::BundleConflict` is at `src/resolve/resolve_error.rs:335`, with caps at `:25-29`.
- Exit mapping: `BundleConflict` maps to `ConfigError` (78) at `src/error.rs:609-611`.

## 2. Pass loop today

**Two nested loops.** Your brief says "pass loop in `Graph::expand`". It is actually two loops.
- `Graph::expand` (`graph.rs:1497`) is one wave-by-wave walk over the roots.
- The outer pass loop is `walk_until_settled` (`:1905-1922`). It calls `expand`, then `settle_pass`, and if that returns true it calls `restart` and runs again.
- `settle_pass` (`:1929`) is `attest_direct_claims() || release_conflicts()`.
- `expand_counting_passes` (`:1843-1885`) may run a second whole walk (`kept`, with `confirmable` and `attested`) to keep pins a typed line's edges resolved onto.

**How a node gets its digest.**
1. `seed` (`:1223`) sorts roots by `(kind, name)` and inserts every registry root as a `Pending` with `via: None`. A path root is read at once from its own edges.
2. `resolve_digests` (`:1276`) works through the pending list.
   - It sorts by `(probe, key, via, id)` (`:1284`).
   - It skips an identifier when `digest_of` already has one, or the identifier was seen this call (`:1289`).
   - Otherwise it spawns `retry_chain` (`:1293`; `resolver.rs:1312`), which calls `access.resolve_digest(id, Resolve)`. A tag is resolved once per run: `digests: BTreeMap<Identifier, Digest>` is keyed by identifier and survives `restart` (`:1562-1566`).
   - The retry chain is bounded by `RESOLVE_CONCURRENCY = 32` (`resolver.rs:1306`) through `run_bounded` (`graph.rs:403`).
3. `digest_of` (`:1084`) tries the carried pin first (`carried`, `:938`, which returns `None` if the key is in `released`), then `digests`.
4. `schedule` (`:1359`) turns touched keys into `Job`s.
   - `settle` decides the digest source (`:1365-1386`). A key in `Conflict` is skipped, so a conflicted node is never read.
   - It sets `queued = true` (`:1406`), so a node is read once per pass.
5. `expand` (`:1511-1523`) reads each job with `read_node` (`:611`) and then folds the reads. `read_node` calls `read_registry` (`:490`).
   - `read_registry` fetches the manifest by digest, so there is no second tag resolve (`:499-504`).
   - It then fetches the requires or members layer by descriptor.

**Folding edges.** `fold` (`:1423`) turns each read's edges into `Claim`s on child nodes (`:1466-1473`) and into new `Pending` digest requests (`:1480`). An edge onto a direct key is not resolved unless it is a probe (`:1477-1480`).

**Re-reads.**
- Within a pass, never.
- Across passes: `restart` (`:1567`) clears `nodes` and keeps each manifest read in `Reads`. `Reads::take` (`:306`) hands it back only for the same id and digest.
- A gone manifest or an offline stand-in has `from_manifest: false` (`:287`), so it is asked again in every pass.

**What `released` does.** `release_conflicts` (`:1543-1560`) inserts a conflicted key into `released` when two things hold at once: a carried pin answers one of its claims, and another claim is neither answered nor recorded in the previous lock. After release, `carried` and `kept` return `None` for that key, so it resolves fresh. A conflict among recorded edges releases nothing, so a moving tag cannot move a pin outside `grim update`.

**What ends the passes.** `settle_pass` returns false when neither `attested`/`unattested` nor `released` grew (`:1059-1082`, `:1543`). `expand_graph`'s docs bound it at `3 * carry.pins.len() + 1` passes (`:1806-1808`). I found no counter enforcing that bound. It follows from the sets only growing.

**Determinism.**
- Roots sorted at `:1224`.
- Pending sorted at `:1284`.
- Reads sorted by key at `:1522`.
- `run_bounded` returns results in submission order (`:422`).
- `touched` is a `BTreeSet`.
- `settle` sorts and dedups edges (`merge_policy.rs:64-66`).
- The winning id is the smallest request (`:58-60`).
- `resolve_members` sorts edges (`:484`).

**Node cap.** It is enforced in `fold` before admission.
- Bytes: `:1443`. Depth: `:1446`. Nodes: `:1450`.
- A declared bundle's members are exempt (`exempt = is_declared_bundle`, `:1427`).
- The counters reset in `restart` (`:1578-1579`), so each pass counts from zero.
- Constants are in `resolve_error.rs:25-29`. Breaches become `GraphTooLarge`, exit 65.

**After the passes.** `finish` (`:1583-1595`) reports conflicts first (78), then cycles (65), then builds the graph.

## 3. Claims and agreement

All three rules live in `settle` (`merge_policy.rs:63-98`).
- **Direct wins** (`:67-86`): any direct claim returns `Settled::Direct`. Edges whose digest differs are listed as `shadowed`. Edges naming another registry or repository are listed as `publisher_mismatch`. This becomes `GraphWarning::PublisherMismatch` in `graph.rs:1744`.
- **Equal digests merge** (`:88-93`): `edges.iter().all(|e| e.digest == first.digest)` gives `Agreed`. The winner id is the smallest request.
- **Different digests conflict** (`:96`): `Conflict { contributors }`. This includes a known digest beside an unknown one.

**The conflict message.**
- Template: `resolve_error.rs:329-335`, "conflicting versions ({sources}){others}; declare {kind} {name} directly in grimoire.toml to choose one".
- Built in `Graph::conflict_error` (`graph.rs:1611`). Per-contributor text is built by `contributor` (`:1598`): `{id} from bundle {repo}:{tag}` or `{id} required by {kind}/{name}`.
- Further conflicts are appended as `others` (`:1620-1633`).
- Marketplace parts use `PluginConflict` instead (`resolve_error.rs:353`).

**Digest agreement is the only definition.** Claims carry a digest, never a tag. A tag only matters through its resolved digest.

## 4. Facts to verify

**a. Version annotation.**
- Stamped on every kind: yes. `org.opencontainers.image.version` is inserted unconditionally in `annotations_for_skill/rule/agent/bundle/mcp/settings/hook` (`src/oci/annotations.rs:420,512,587,680,742,805,868`).
- Value for a non-semver `--version`: the raw release tag. `release.rs:217` sets `version = id.tag()`, which is passed to the annotation builders (`release.rs:437,525,611`). So `canary` gives `"canary"`, and `1.2` gives `"1.2"`. It is not normalized.
- Read during resolve: no. There are zero hits in `src/resolve`, `src/lock` or the commands. It is read only for `describe` (`src/fetch.rs:1076`) and export (`export/stage.rs:39`).
- Read-path hook: `read_registry` already holds the `OciManifest` (`graph.rs:501`), and `OciManifest.annotations` exists (`src/oci/manifest.rs:73`).
- Gap: the acceptance fixtures `make_dependent` and `make_artifact` do not stamp it (`test/src/helpers.py:170,341`; `make_dependent` passes only `com.grimoire.kind` plus caller extras).

**b. `list_tags`.**
- Signature: `async fn list_tags(&self, id: &Identifier) -> Result<Option<Vec<String>>, AccessError>` (`src/oci/access.rs:134`). `Ok(None)` means the repository is unknown.
- Pagination: follows the `Link` header (`registry_client.rs:667-717`). It stops at 128 MiB of tag names (`CATALOG_LISTING_BYTE_LIMIT`, `access.rs:33`). It also stops on a repeated cursor or an empty page with a next link, returning the tags so far with a warning.
- Offline: `CachedAccess` returns `tags.get_tags(id)` (`cached_access.rs:205-208`). That is only the tag names previously resolved (the cache file maps tag to digest, `tag_cache.rs:140-148`; `put_tags` only prunes). So offline it lists just the tags this machine resolved before, and `Ok(None)` if there is no cache file.
- Online it persists via `put_tags`.
- Callers: `describe` (`fetch.rs:1027`), announce (`catalog/index_announce.rs:456`) and the catalog (`catalog/registry_catalog.rs:1380`). The resolver never calls it. `Operation` does not apply: `list_tags` takes no `op`, so there is no Query/Resolve distinction.
- `MemoryRegistry::list_tags` (`memory_registry.rs:82`) works as a test double.

**c. Semver parsing and shape classifier.**
- `semver = "1"` is already a dependency (`Cargo.toml:49`).
- Reusable pieces: `publish_tags` and `cascade_set` (`src/oci/release.rs:149,197`). They define the cascade shape as `X.Y.Z`, `X.Y`, `X`, `latest`.
- Also `pick_highest_version` (`registry_catalog.rs:1581`), which ignores `latest` and maps `_` to `+`, and `pick_latest_tag` (`:1544`).
- The `is_reserved_float_tag` classifier is used in `publish.rs:841-860`. I did not read its body.
- Gap: there is no parser for a tag as a claim. `1.2` is non-semver to `semver::Version::parse`, and `release.rs:54-55` documents that `1.2` publishes as a literal. `^`/`>=` requirement logic is not used anywhere. `semver::VersionReq` is available in the crate but nothing uses it.
- Cascade moves unconditionally: `move_tags` (`release.rs:824-836`) puts `X.Y`, `X` and `latest` on whatever was just published, so releasing an older patch moves `2` and `latest` backward.

**d. `or_latest`.**
- Definition: `src/oci/identifier.rs:270`. It adds `latest` only when there is neither tag nor digest. `tag_or_latest` is at `:262`.
- Callers outside tests:
  - `fetch.rs:327,802,1022`
  - `export/resolve.rs:220`
  - `export/marketplace.rs:483,897`
  - `command/add.rs:1851`
  - `config/project_config.rs:1001`
  - `config/toml_merge.rs:177`
  - `oci/member_ref.rs:134` (`with_default_tag_latest`)
- Where a bare reference is normalized: `project_config.rs:1001` (the `grimoire.toml` artifact tables), `add.rs:1851` (the `grim add` argument) and `project_config.rs:1039` (members declared in config).
- Edges inside published layers are not normalized. `resolve_members` (`graph.rs:464-478`) calls `MemberRef::parse` and `resolve`, never `or_latest`. A tagless edge id reaches the registry client, which treats it as `latest` (`registry_client.rs:212`; `tag_cache.rs:100,116`). The lock records it as a tagless pin.
- Consequence: `or_latest` happens at parse. `DesiredSet` holds `:latest` for a bare declaration, and that form is what the declaration hash is computed from (`declaration_hash_cached`, `prior.rs:49`). I did not trace the hash's JCS input, so "bare means any version" cannot be told apart from an explicit `:latest` after parse. I did not confirm whether the hash covers the normalized id.

**e. What the lock records per node.**
- `pinned` keeps the requested tag. D1 records `registry/repo:tag@sha256:…` (`adr_lock_pin_stability.md:117-119`). A digest-only request stays tagless. For a transitive node the tag is the winning edge's request.
- No version is recorded anywhere. `LockedArtifact` has `name`, `kind`, `source`, `bundles` and `requires` (`locked_artifact.rs:46-71`). The only version-like fields are `lock_version`, `declaration_hash_version` and `generated_by`.
- An optional field would be additive for new grim but not for old readers:
  - `RawLockedArtifact` is `#[serde(deny_unknown_fields)]` (`locked_artifact.rs:79-80`). Old grim exits 78 on a lock holding the new key, exactly as with `requires` (ADR Decision 4: "grim ≤ 0.15.1 reading a lock with either field exits 78").
  - `RawLockedBundle` has `deny_unknown_fields` (`locked_bundle.rs:146-147`), as does `LockMetadata` (`grimoire_lock.rs:28-29`). The schema must gain the property (`docs/public/schemas/grimoire-lock.schema.json`).
  - Pin-stability ADR (`:112`) says a new field is ruled out by Principle 9 for the residual case.

**f. Install scope and `--force` scope.**
- `grim add X`:
  - Refresh is `Nothing` (`add.rs:374`). The lock is saved (`:396`) before any install.
  - Only the target's reach installs. `Added::new` takes `projection = reachable_lock(new_lock, &target)` (`add.rs:920`). `install_added` installs that projection (`:1242-1250`).
  - `--force` is `ForceScope::Only(added.force)`, where `added.force = force_nodes(new_lock, &target)` (`:930`, `:1279`). That is the target, or a bundle's members, never a dependency it merely requires.
  - A moved pin outside the reach is written to the lock and not installed. It shows in `moved` and `dropped` (`add.rs:313-314`) and in a stderr line. A forced refusal on a dependency exits 65 naming it (`:1330` onward).
- `grim update X`:
  - `NamedScope` (`update.rs:688-701`) holds `install = closure_nodes(new_lock)` and `prune = closure_nodes(previous)`.
  - `--force` is `ForceScope::Only(install ∪ prune)` (`:719` area, `:1056-1062`).
  - The lock is saved before install (`:183`).
  - Install goes through `project_lock(&new_lock, &n.install)` (`:237-239`). `pass_writes` gates which outputs count as writes (`installer.rs:1939`).
  - A node outside the closure whose pin moved: the lock changes, the files do not. `warn_left_behind` (`update.rs:740-770`) says "the lock also changed … outside the named closure, and their installed files stayed as they were; `grim install` applies a new pin". The node also gets a report row via `changed_outside` (`:714-718`).
- Bare `update` is `Refresh::All`, with no scoping.

**g. Pin moves and the installed files were edited.**
- Gate: `installer.rs:1941-1954`. The on-disk hash differs from the recorded one and `pass_writes` is true, with no `force`. Result: `InstallOutcome::Refused { recorded, actual }`.
- Error and exit: `InstallErrorKind::IntegrityMismatch` (`install_error.rs:117`), mapped to `DataError` (65) with reason `LocalModified` (`error.rs:622-625`).
- Lock timing: written first. In `update`, `lock_io::save` runs at `update.rs:183`, before the install. In `add`, `lock_io::save` is inside `write_config_and_relock` (`add.rs:396`), before `install_added`. So a refusal exits 65 with the new pin already locked and the file untouched. The comment at `update.rs:213-220` says this.
- A refusal outside `force` is reported by `refusals_outside_force` (`add.rs:~885`).

**h. Offline.**
- Tag resolve offline: `CachedAccess::resolve_digest` reads the tag cache. On a miss with `Resolve` it returns `OfflineMiss` (`cached_access.rs:70-77`).
- Exit code: `retry_chain` wraps it as `RegistryUnreachable` (`resolver.rs:1325-1330`). `classify` takes the first `Error` in the chain, which is `Error::Resolve`, so `classify_resolve` (`error.rs:594`) gives 69. The 81 mapping at `error.rs:564` is for a bare `Error::Access`. I inferred this from the code and did not run it.
- A carried node offline: its manifest is read by digest from the store. On a store miss `offline_members` (`graph.rs:887-897`) stands in the lock's recorded `requires` at that digest, or none, with a warning (`offline_read`, `:588-607`). A bundle needs a row with members, else the read fails.
- A moved pin through the tag cache also warns from `target_lock`.
- Gap for the new design: `list_tags` offline returns only previously resolved tag names (see 4b), so it cannot discover a newer version offline.

**i. Flags per command.**

| Command | Own flags | Source |
|---|---|---|
| add | `--kind`, `--name`, `--force`, `--[no-]install`, `--trust-workspace`/`--no-trust-workspace` | `add.rs:74-127` |
| update | `--force`, `--client`, `--marketplace` (conflicts with `--force` and `--client`), trust pair | `update.rs:81-110` |
| lock | none (`LockArgs {}`) | `lock.rs:42` |
| install | `--force`, `--client`, `--kind` (requires `path`), trust pair | `install.rs:35-70` |

- `--offline` is global: `GlobalOptions.offline` (`src/cli/options.rs:78`), read through `ctx.offline()`.
- Where a shared flag would go: `HookTrustOpts` (`options.rs:146-170`) is the existing shared flattened `Args` struct. It is flattened with `#[command(flatten)]` at `add.rs:105`, `update.rs:108` and `install.rs:67`. A new struct would flatten the same way. `lock` would need a field added to its empty struct.
- `install` does not resolve. It only calls `require_fresh_lock` (`install.rs:91`, `:442-460`) and materializes. Putting `--allow-downgrade` on `install` has nothing to act on unless `install` is meant to re-resolve.
- A resolve flag would need plumbing into `target_lock`. Every caller passes `ResolveOptions::default()` (`add.rs:376`, `update.rs:146`, `lock.rs:67`). `ResolveOptions` (`resolve_options.rs:25`) has only timeout and retry fields.

**j. Dispositions.**
- Computed by `plan_lock(previous, target)` (`disposition.rs:39-62`), a pure diff. A node absent before is `Added`. A node with the same content (`eq_content`, tag ignored) is `Carried`. A different content identity is `Moved`. A node gone is `Dropped`. A stamp or a tag-only change is `Carried`.
- Bundles have no row. `warn_moved_bundle_pins` (`lock.rs:94`) prints one stderr line per moved `[[bundle]]` pin.
- `lock` reports `{kind, name, pinned, action, disposition}` as `items` (`api/lock_report.rs`).
- `add` reports `moved`, `dropped` and `pulled_in` as `PinnedNode` lists (`api/add_report.rs:47-65`), from `moved_pins` and `dropped_pins` (`add.rs:722,744`).
- `update` maps dispositions to `UpdateAction`. `Carried` is `Unchanged`, or `Restored` if the pass rewrote the files. `Added`/`Moved` are `Updated`. `Dropped` is `Removed` (`update.rs:1390-1394`).

## 5. Reusable code and fixtures

**Unit and in-process tests.**
- `Fake` registry and `Pub` in `src/resolve/graph.rs` (`:1951`, `:1987`). It counts resolves, manifest fetches and blob fetches. It supports tag failures (`Failure`), offline store misses, and `alias_tag` (`:2841`).
- `Pub.annotations` (`:1992`) can carry the version annotation. Helpers: `pubd` (`:1995`), `direct` (`:2177`), `run` (`:2193`), `fetches_of` (`:2847`).
- Existing diamond and conflict tests: `equal_digests_behind_different_tags_merge_into_one_read` (`:2916`), `different_digests_conflict_and_the_message_says_how_to_choose` (`:2945`), `every_further_conflict_is_reported_once` (`:2996`), `a_conflicted_node_is_never_read` (`:3154`).
- Pass-loop tests (`:4443-4730`): `disagreeing_on_x`, `each_pass_releases_what_the_last_one_exposed` (`:4478`), `a_release_pass_reads_only_what_the_release_changed` (`:4509`), `the_node_cap_counts_each_pass_from_zero` (`:4636`), `a_release_pass_is_held_to_the_node_cap` (`:4689`).
- Carry builders: `carried_pin`, `carried_requires`, `held_at`, `walked`.
- `MockAccess` (`resolver.rs:1418`) is a scripted VecDeque of responses with call and read counters. Wrapper doubles that forward to a registry (`list_tags` forwarded at `resolver.rs:3984`).
- `MemoryRegistry` (`src/oci/access/memory_registry.rs`) is the in-memory registry. It supports `put_tag`, `list_tags` (`:82`), `push_blob` and `push_manifest`.
- Agreement harness: `src/resolve/agreement_tests.rs` (online expander against the offline closure on one published fixture; `fixture`, `online_view`, `offline_view`). `src/resolve/pin_agreement_tests.rs` (6321 lines) crosses lock shapes with edits and tag moves through the real writers. Its `Store` double (`:61`) records resolves and tag history.
- Pure-rule tests for `settle`: `merge_policy.rs` tests include a `permutations` helper for order-independence.

**Acceptance tests** (`test/tests/`).
- `test_dependencies.py`: `test_diamond_equal_digest_merges` (`:135`), `test_diamond_conflict_lists_all` (`:151`), `test_direct_overrides_edge` (`:171`), `test_cycle_refused`, `test_offline_lock_carries_forward` (`:361`), `test_offline_lock_command_carries_forward` (`:400`). It uses `_leaf` and `_dependent` (`:24`, `:30`).
- Also `test_dependencies_add.py`, `test_dependencies_install.py`, `test_dependencies_mutations.py`, `test_dependencies_contract.py`, `test_dependency_fixtures.py`, `test_lock_edges.py`.
- Pins: `test_lock_pins.py` (`test_lock_keeps_pins_after_a_tag_moves` `:164`, `test_tag_edit_moves_only_that_pin` `:581`, `test_update_named_moves_a_shared_reference` `:677`), `test_old_grim_lock_pins.py`.
- Scope: `test_update_scope.py` (`test_named_update_reports_a_moved_pin_outside_its_closure` `:419`, `test_force_overrides_the_closure_only` `:373`, `test_modified_dependency_in_the_closure_exits_65` `:358`).
- Publishing helpers: `test/src/helpers.py:170,341`; `test/src/registry.py:209` (`push_manifest`), `:327` (`retag`). `retag` models a moved cascade tag.
- `test_release_nonversion.py` covers channel tags.

## 6. Obstacles in the current code

1. **Tag equality is the pin rule's identity.**
   - `PinnedIdentifier::answers` (`pinned_identifier.rs:46-51`) requires `self.0.tag() == requested.tag()`. `answers_direct`, `answers_edge` and `keeps_pin` (`pin_rule.rs:391-445`) all build on it.
   - A pin recorded as `x:2@D` will not answer a claim `x:2.1`, and no version is stored (4e) to test satisfaction. "A pin stays while it satisfies every claim" needs version information the lock lacks. It would have to come from the manifest annotation (4a) read by digest, from the lock, or from the cascade tag, and none is wired in.
2. **Claims carry digests, not tags.** `EdgeClaim` and `DirectClaim` (`merge_policy.rs:19-34`) hold the requested id and a digest. `settle` only compares digests. A major or lower-bound claim would need to ride on the claim, and `settle` has no place for it.
3. **The release set is only per-key and only at conflict time.** `released` (`graph.rs:905`) holds keys, not constraints. `release_conflicts` (`:1543`) fires only on a conflict between an answered and an unanswered unrecorded claim. Sticky learned constraints are new state, and `restart` (`:1567`) clears `nodes` while keeping only `digests`, `reads`, `released`, `attested` and `unattested`. Constraints would have to survive `restart` as `released` does.
4. **The termination argument rests on monotonic key sets.** The `3 * pins + 1` bound (`graph.rs:1806-1808`) assumes each pass adds a key to `released`/`attested`/`unattested`. A constraint that tightens a version per node is a larger lattice, and no pass counter exists (`walk_until_settled`, `:1905-1922`).
5. **A tag is resolved once per identifier, from `digests` (`:920`).** `resolve_digests` skips any identifier already in `digests` (`:1289`), and `digests` is kept across restarts (`:1565`). Re-choosing a version for a node under a new constraint means a different identifier (`x:2` vs `x:2.1.0`) or a change to this memo.
6. **`or_latest` cannot be undone after parse.** Config, `grim add` and member parsing turn bare into `:latest` (4d). After that, "no tag" and `:latest` look the same, and the declaration hash may cover the normalized form. Also, in published layers a tagless id is what a digest-only edge looks like too (`pin_rule.rs`: a tagless pin means digest-only or pre-change, `adr_lock_pin_stability.md:130`). "Tagless = any version" collides with "tagless pin = digest request or pre-change lock". Existing locks hold `:latest` pins that would then read as an any-version claim.
7. **`list_tags` has no resolver path and is weak offline.** It is never called from `src/resolve`, takes no `Operation`, and offline returns only tags previously resolved (4b). It has no per-repo timeout and no `retry_chain`-style handling like `resolve_digest`. The 128 MiB cap is large, so a repo with many tags gives a big list.
8. **Cascade tags are not a reliable "newest in major".** `move_tags` (`release.rs:824-836`) moves `X`, `X.Y` and `latest` on every release, including a backport. `2` can point at an older version than `2.9.0`. The decided design says list_tags is only a fallback for a missing tag, so a stale cascade tag is not detected.
9. **`--allow-downgrade` on `install` has no home.** `install` never resolves (`install.rs:91`). On `lock` the args struct is empty (`lock.rs:42`), and no flag reaches `target_lock` (`ResolveOptions::default()` at `add.rs:376`, `update.rs:146`, `lock.rs:67`). The TUI calls `target_lock` at four sites (`tui/app.rs:3549,4083,8998,9406`) and the export stage once, so they also need a value.
10. **Direct tagless yields to a major claim.** Today `Settled::Direct` ignores every edge claim (`merge_policy.rs:67-86`) and `grim lock` keeps a direct pin even over a recorded edge. Making a tagless direct yield needs the direct claim to say whether its tag was typed. That information is lost by `or_latest` (item 6), and `Prior`'s `declared` field, which exists for "typed line" logic (`graph.rs:706`, `attested`), is already complicated by the attestation machinery (`confirmable`, `stamps_declared`).
11. **The "moved by an unchosen version" exit has no data.** The graph reads each node once per pass under one winner (`schedule`, `:1359`). It has no record of which parent version contributed an edge once that parent is dropped, because claims live on `Node` and `restart` discards them (`:1569-1575`). Claims from a version that was later not chosen would need to be kept across restarts.
12. **Test fixtures lack version stamps.** Acceptance helpers write only `com.grimoire.kind` (`test/src/helpers.py:188-191`), and `make_artifact` goes through `push_artifact`. Version-based tests need annotations added. The in-memory `Fake` and `MemoryRegistry` support annotations but the existing tests set none.

**Not checked:** `is_reserved_float_tag` body, the exact declaration-hash input, and the TUI call sites' behavior. Nothing was run, so the exit-code claims for offline (69 vs 81) rest on reading `classify` and are not confirmed by a test run. Risk to know: items 1, 3 and 6 are structural, because the pin rule and the claim types assume tag equality.
