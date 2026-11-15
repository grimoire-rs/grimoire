# Research: dependency version claims, implementation surface

- **Date:** 2026-11-12
- **Expires:** 2027-05-12
- **Source:** four explorer passes for `/hex-plan` (tier high) on `.agents/adr/adr_dependency_version_claims.md`
- **Note:** line numbers are as of commit `32b8d4b6`; verify before relying on one. Reports were written before the ADR's final text pass, so 'Q1/Q2/Q3' mentions are answered: Q1 option a, Q2 option a, Q3 F1.

---

# Part: File ownership and layering

Read-only mapping is done and nothing was edited. Graph.rs and resolver.rs line numbers come from this worktree. The ADR was re-read after the first pass and looked consistent. The cited research map was not redone.

## 1. File ownership per contract

**a. Claim grammar and line intersection (new pure module)**
- The only layout rule is `.claude/rules/arch-principles.md:107` ("One concept per file; named module files, no `mod.rs`"). The nearest precedents are:
  - `src/lock/merge_policy.rs:1-12`, a pure rule that takes no registry access.
  - `src/oci/requires_decl.rs:4`, the grammar of a `requires` declaration.
  - `src/oci/release.rs:149,197`, where tag shape and `semver` already live (`publish_tags`, `cascade_set`).
- Dependency direction is `oci` (leaf) <- `lock` <- `resolve`. The `src/lock` -> `src/resolve` imports in `locked_bundle.rs` and `stored_members.rs` were not checked.
- A grammar used by `merge_policy`, `pin_rule` and `graph` therefore has to sit in `src/oci/` or `src/lock/`, not `src/resolve/`. `src/resolve/` is the walker and depends on `lock`. The decision is the plan's, but `src/oci/` (new named file, e.g. beside `release.rs`, registered in `src/oci.rs:7-28`) matches `T(line)` being the inverse of `cascade_set`. A `src/lock/` file matches `merge_policy` and `pin_rule`.
- Items: `claim_of`, `Claim`/requirement set, line intersection and empty-line test, `T(line)`, `highest_in_line` (ADR 285).
- Existing tests: none cover the grammar. `semver::VersionReq` is unused today. Tests for the shape reuse are `src/oci/release.rs:213-320` and `is_reserved_float_tag` at `src/command/publish.rs:3680`.
- `is_reserved_float_tag` (`publish.rs:873`) and `cascade_set` (`oci/release.rs:197`) are private. Leave them untouched.

**b. Evidence function and memo**
- The pure half is `evidence(digest, reference)` and `satisfies(evidence, claims)`. Put it in the same new module as (a), or one beside it. The annotation key literal is only in `src/oci/annotations.rs:420,512,587,680,742,805,868`.
- The memo half (`evidence: BTreeMap<Digest, Option<Version>>`) is a field on `Graph` (struct at `graph.rs:900`). It is filled where the manifest is already in hand in `read_registry` (`graph.rs:490-504`), via `OciManifest.annotations` (`src/oci/manifest.rs:73`).
- The pin side is `CarriedPin` / `EdgeEvidence` (`graph.rs:701`; `pin_rule.rs:121`). An offline store read uses `offline_members` (`graph.rs:887`).
- Existing tests: `Pub.annotations` in the `Fake` registry (`graph.rs:1992`) supports annotations, but no test sets one. Acceptance fixtures do not stamp the annotation (`test/src/helpers.py:170,341`).

**c. `settle` split**
- `settle` is at `src/lock/merge_policy.rs:63`, with `DirectClaim` (`:19`), `EdgeClaim` (`:28`), `Settled` (`:38`) and `winning_request` (`:58`). Add a Range, Literal or Digest claim shape here and the validation half of the split. The selection half cannot go here because it needs registry access, so it lands in `graph.rs` (d).
- Callers of `settle`, exactly two:
  - `src/resolve/graph.rs:1343`, inside `Graph::settled` (`:1341`). Its consumers are `schedule` (`:1365-1385`), `conflicts` (`:1532`), `settle_winners` (`:1715`) and `into_graph` (`:1744`).
  - `src/lock/effective_set.rs:369`, inside `settle_origin` (`:346`). It builds `EdgeClaim`s with `digest: locked`, matches only `Agreed`, and treats `Conflict | Direct` as `None`.
- Other users of the types: `winning_request` is also used at `src/lock/pin_rule.rs:18,237`. `claims_of` (`graph.rs:1317`) builds the claims.
- Existing tests:
  - `src/lock/merge_policy.rs:145-286`: ten unit tests, including a `permutations` helper.
  - `src/lock/effective_set.rs`: 50 tests, starting at `:608`.
  - `src/resolve/agreement_tests.rs`: 3 tests.
  - `graph.rs` settlement region `:2838-3040`.

**d. Target ladder and tag listing in the graph (`graph.rs`)**
- All of these are in the non-test part, lines 1-1933. Fields for learned lines, evidence memo, listings and the work counter go on `Graph` (`:900`, constructor `Graph::new` at `:1822`). `Claim` (`:253`) and `Node` (`:316`) gain the claim shape.
- Regions per function:
  - `resolve_digests` `:1276`. It currently skips any identifier already in `digests` (`:1289`) and runs through `retry_chain` (`resolver.rs:1312`, `pub(super)`). The `T(line)` resolve and the listing call go here or beside it.
  - `claims_of` `:1317`, `settled` `:1341`.
  - `schedule` `:1359`. This is where the selection ladder, held pin first, enters.
  - `fold` `:1423`. It builds claims from edges (`:1466`), and its node, byte and depth caps are at `:1443-1451`.
  - `release_conflicts` `:1543`, `restart` `:1567` (keeps `digests`, `reads`, `released`, `attested`, `unattested`), `finish` `:1583` (Decision 7 checks and the downgrade guard run last).
  - `walk_until_settled` `:1905`, `settle_pass` `:1929` (currently `attest_direct_claims() || release_conflicts()`), `expand_counting_passes` `:1843` (the kept walk builds a fresh `Graph` at `:1864`, which is where a shared work counter and copied evidence and listing memos would go).
- Existing tests in `graph.rs` (`tests` starts at `:1935`, 91 tests):
  - expansion `:2212`
  - caps `:2646`
  - settlement `:2838` (including `equal_digests_behind_different_tags_merge_into_one_read` at `:2916`)
  - cycles `:3040`
  - precedence `:3105`
  - carry `:3174`
  - release and pass loop `:4433-4730`
  - part stamps `:4896`
- `src/resolve/agreement_tests.rs` and `src/resolve/pin_agreement_tests.rs` also cover the walker.

**e. Pin answers rule and its seam**
- Leaf: `PinnedIdentifier::answers` (`src/oci/pinned_identifier.rs:46`). It requires tag equality. It has 10 unit tests (round at `:250`). Do not change it.
- Shared by two callers: `PinRecord::names` (`src/lock/pin_rule.rs:48-54`) calls `pin.answers`, and `answers_direct`, `answers_edge`, `keeps_pin` and `restamp_answers_declared` (`:462`) sit on top of it. A Range row put in `names` would leak into `edit.rs` and `add.rs`. The ADR's "graph carry only" means the row goes beside these, not inside `names`.
- Seam in `graph.rs`:
  - `Carry::answer` (`:774`) and `answered` (`:787-792`) dispatch on `NodeClaim` (`:738`). A new Range claim variant or row goes here.
  - `carried_pin` (`:800`), `records_request` and `kept` (`:851-859`, which delegates to `pin_rule::keeps_pin`) are the other readers.
- `part_bundles.rs:840` also calls `pin_rule::answers_edge` (a fifth consumer to keep byte-for-byte).
- Three callers that must stay on tag equality:
  - `src/lock/edit.rs:1205-1234`, via `pin_rule::restamp_answers_declared` and `restamp_answers_edges`. They also use `stamp_kept_pins` at `:892` and `snapshot_matches` (imported at `:15`).
  - `src/command/add.rs:823` (`answers_direct`) and `:840` (`pin.answers`).
  - `src/export/resolve.rs:226` (`pin.answers`).
- Tests: `pin_rule.rs` has 23 unit tests, `edit.rs` 167, `pin_agreement_tests.rs` 31 `#[tokio::test]`/`#[test]` (it also has many table-driven rows).

**f. Work budget and listing budget**
- `src/resolve/resolve_error.rs:25-29` holds the caps (`MAX_TRANSITIVE_NODES`, `MAX_REQUIRES_BYTES`, `MAX_DEPTH`). `GraphLimit` (`:33`) gains `Work` and `ListingBytes`. Its `Display` is at `:42`. `GraphTooLarge { limit, via }` is at `:415`. The "node reads" message is a new `via`/message form, so a new `GraphLimit` arm or a variant change.
- Enforcement lives in `graph.rs`: the node, byte and depth caps in `fold` (`:1434-1451`), which reset per pass in `restart` (`:1578`). The new work counter must not reset. The listing byte cap applies per walk at the 128 MiB per-repository cap (`src/oci/access.rs:33`).
- Tests:
  - `resolve_error.rs:705-731` checks the `GraphLimit` display strings.
  - `error.rs:1965-1973` covers `GraphTooLarge` classification (65, `error.rs:605`).
  - `graph.rs:2699,2740` plus pass-loop caps tests at `:4636,4689`.
- Adding a `ResolveErrorKind` variant: `classify_resolve` (`src/error.rs:586-612`) is an exhaustive match, so every new variant must be mapped there.

**g. `grim release` float rule**
- Today `publish_tags` (`src/oci/release.rs:149`) builds the full set `[X.Y.Z, X.Y, X, latest]` from `cascade_set` (`:197`) from the tag alone. `move_tags` (`src/command/release.rs:822-836`) then does `put_tag` for the exact tag, then every other tag, with no read of what the float currently names.
- Callers of `move_tags`: `run` `:361`, `release_bundle` `:472`, `release_mcp` `:563`, `release_settings` `:649`. All four build `ReleaseReport::new(..., tags, true)` from the planned set (`:369,474,565,651`), and the dry-run reports at `:334,460,548,634`. So a skipped float would still appear in `tags` unless those sites change.
- `ReleaseReport.tags` is documented as "The cascade tag set pointed at the manifest" (`src/api/release_report.rs:31`).
- Reuse by `grim publish`: `publish.rs:1289` calls `super::release::run(ctx, &release_args)` per entry. It forwards `cascade` and `no_cascade` at `:1280-1281`. The report goes through `publish_entry_from_release` (`:2657`, `tags: report.tags.clone()` at `:2666`). Status is derived from the report: `report.pushed` -> Pushed, `report.tags.is_empty()` -> Skipped, otherwise DryRun (`:1297-1303`). A float rule that empties or shortens `tags` would shift that classification. The rule is inherited automatically with no `publish.rs` edit if it lives in `release.rs`. The skip-existing path returns before the dry-run branch (comment at `publish.rs:4764`).
- Tests covering cascade moves:
  - Unit: `release.rs:1079` `memory_registry_release_pushes_cascade_idempotent_and_guards` (only unit test calling `move_tags`, at `:1096`). `oci/release.rs:213-320` cover `publish_tags` shape only. `publish.rs:3649,3680,4562` cover channel and reserved-float validation.
  - Acceptance: `test/tests/test_release.py:114` (`test_release_pushes_with_cascade_tags`), `:149` (`test_rerelease_moves_preexisting_floating_tags`, upward only), `:289` (prerelease exact only), `:328-330` (`1.0.1 --skip-existing` yields `["1.0.1","1.0","1","latest"]`). Also `test/tests/test_release_nonversion.py:31-143`, and in `test_publish.py` `:327,486,556,775`.
  - No test, unit or acceptance, releases a lower version after a higher one (grep for backport, older, lower found nothing relevant). The new float rule needs one.

## 2. Shared files

| File | Contracts | Regions | Independence |
|---|---|---|---|
| `src/resolve/graph.rs` (5172) | b, c, d, e, f | b: memo field plus `read_registry` `:490-504`. c: `claims_of` `:1317`, `settled` `:1341`, `schedule` `:1365-1385`, `settle_winners` `:1709`. d: `resolve_digests`, `schedule`, `fold`, `restart`, `finish`, `walk_until_settled`, `expand_counting_passes`, `settle_pass`. e: `NodeClaim` `:738`, `Carry::answer` `:774`, `kept` `:851`. f: `fold` caps `:1434-1451`, `Graph::new` | Same file, same `Graph` struct and `restart`. Serialize. Split only by the ladder, carry and budget regions if each pipeline owns a stub-first set of private fields. |
| `src/resolve/resolve_error.rs` | f (limits), errors (`NoMatchingVersion`, `UnchosenRequirement`, `MajorDowngrade`, `GraphLimit`) | `:25-47`, `:335`, `:415` | Mostly independent regions, but one enum edit at a time. |
| `src/error.rs` | errors, f | `classify_resolve` `:586-612`, `ErrorReason` `:177-214` (+ slug and tests `:1047-1087,1870-1882`) | Independent of graph work. Serialize with `resolve_error.rs`. |
| `src/lock/merge_policy.rs` | c (and a, for claim types) | types at `:19-56`, `settle` `:63` | c owns it. a must land its types first. |
| `src/lock/effective_set.rs` | c | `settle_origin` `:346-379` only | Single-contract. Touched only by c. |
| `src/lock/pin_rule.rs` | e (and b for `satisfies`) | new rows near `answers_edge` `:402` | e owns it. |
| `src/resolve/pin_agreement_tests.rs` (6321) | all, as validation | `exit-agrees`/`relock-error` around `:3147-3184`, `Store` `:61-158` | Touches Store double (annotations, listings, flag). Serialize, last. |
| `src/resolve/resolver.rs` (7889) | `ResolveOptions` downgrade plumbing, `target_lock` `:154`, `build_carry` `:1158`, `retry_chain` `:1312` (`pub(super)`) | flag wiring only | Late, after d/e. |

The flag (`src/cli/options.rs`, `add.rs`, `update.rs`, `lock.rs`, `tui/app.rs`, `export/resolve.rs:435`) was not asked for as a contract. It is wiring after the guard exists.

## 3. Layering order, pure to wired

1. Leaf, no dependency on the others:
   - (a) grammar, `T(line)`, line intersection.
   - (g) release float rule: it only needs `semver` plus the existing annotation key, and has its own files (`src/command/release.rs`, `src/oci/release.rs`).
   - Error and limit variants in (f), `resolve_error.rs` and `error.rs`.
2. (b) evidence, then `satisfies`. It needs (a)'s requirement type.
3. (c) `settle` validation. It needs (a)'s claim shape and (b)'s `satisfies`. The selection half is graph.
4. (e) pin rows. It needs (b)'s `satisfies` and (a)'s Range claim, then wires `Carry::answer`.
5. (d) ladder, learned lines, judge, restart, budgets. It needs (a), (b), (c), (e) and (f)'s limits.
6. Checks (Decision 7, 4), flag wiring, agreement harness, docs, acceptance tests.

Minimal stub signatures so parallel work can start (names are placeholders):
- `enum Claim { Any, Range(VersionReq set), Literal, Digest }` and `fn claim_of(edge: &Identifier) -> Claim`.
- `struct Line` with `fn narrow`, `fn is_empty`, `fn target_tag() -> String` (`T(line)`), and `fn highest_in_line(tags) -> Vec<(Version, String)>`.
- `enum Evidence { Exact(Version), Span(lo, hi), Unknown, Vouched }` with `fn evidence(annotation: Option<&str>, reference: &Identifier) -> Evidence` and `fn satisfies(&Evidence, &Line) -> bool`.
- `fn validate(direct, edges, candidate, &Evidence) -> Settled` (`Settled` gains the new refusals) beside the existing `settle`.
- `GraphLimit::{Work, ListingBytes}` and the three new `ResolveErrorKind` variants with `classify_resolve` arms.
- `Downgrade { Refuse, Allow }` on `ResolveOptions`.

## 4. Size signals

- Non-test parts: `graph.rs` 1-1933 (tests 1934-5172, 91 tests), `merge_policy.rs` 286, `pin_rule.rs` 994, `pinned_identifier.rs` 267, `resolve_error.rs` 901, `resolve_options.rs` 54, `oci/release.rs` 327, `release.rs` 1799.
- Large files:
  - `pin_agreement_tests.rs` 6321
  - `resolver.rs` 7889
  - `edit.rs` 5829
  - `add.rs` 5918
  - `export/resolve.rs` 5340
  - `publish.rs` 6192
  - `update.rs` 3361
  - `error.rs` 2089
  - `effective_set.rs` 1629
  - `tui/app.rs` 18846
- Tell an agent the region, never the whole file: `graph.rs` (`:490-504`, `:640-870`, `:900-1100`, `:1276-1600`, `:1820-1933`; tests only `:2838-3040` and `:4433-4730`). `pin_agreement_tests.rs` (`:61-158` for `Store`, `:3140-3190` for the agreement verdicts). `edit.rs` is edited at `:1205-1234` only. `error.rs` at `:177-214` and `:586-612`.
- Acceptance: `test_release.py` 411, `test_publish.py` 1752, `test_dependencies.py` 425, `test_lock_pins.py` 1646.

## 5. Binding project rules

- `.claude/rules/arch-principles.md:107`: one concept per file, named module files, no `mod.rs`. So the new pure module is a new named file, not an addition to `merge_policy.rs` or `release.rs`.
- `arch-principles.md:106`: omit `#[non_exhaustive]` on internal non-error enums, so `Claim`, `Evidence` and `Downgrade` stay exhaustive. Error enums are exempt.
- `arch-principles.md:108`: domain types over `String`, so use `Version` and `Digest`, not strings, for evidence and lines.
- `arch-principles.md:120-126` (Utility Discipline): check `std` and existing utilities first. `pick_highest_version` and `is_valid_oci_tag` (`identifier.rs:446`) already exist, and the ADR keeps them unchanged.
- `quality-rust.md:71`: `pub(crate)`/`pub(super)` is a design smell, relevant because `retry_chain` is `pub(super)` and `stored_members` is `pub(crate)`.
- `quality-rust.md:80`: no boolean parameters, which is why `Downgrade` is an enum.
- `quality-rust.md:167`: test-only helpers go in a separate `#[cfg(test)] impl` block before `mod tests`.
- `quality-rust-errors.md:20,58`: lowercase, no trailing period in `#[error]`. Acronyms keep case. The ADR templates are compatible.
- `quality-rust-errors.md:116-121`: `thiserror`, `#[source]` on wrapping variants, one enum per module. New refusals go on `ResolveErrorKind` in `resolve_error.rs`, not a new enum.
- `quality-rust-exit_codes.md:141`: never `ExitCode::from(N)`. Map through the typed enum.
- `quality-rust-exit_codes.md:147`: no wildcard arm. New variants must be mapped explicitly in `classify_resolve`.
- `quality-rust-exit_codes.md:28,73`: 79 is `NotFound`, 78 `ConfigError`, 65 `DataError`.

Not checked: other `crate::lock`/`crate::resolve` back-references from `src/oci` (`access/error.rs`, `hook.rs`), the TUI and export call-site changes for the flag, and whether the docs pages named in the ADR need edits.

Risk: the placement of the grammar module (`oci` vs `lock`) is a judgement from precedent, not a stated rule. `graph.rs` is the serialization point for b, c, d, e and f, so those cannot be cut into truly file-disjoint pipelines without stubs first.

---

# Part: Tests and fixtures

# Test surface for `adr_dependency_version_claims.md`

All paths are under `/home/mherwig/.herdr/worktrees/grimoire/uno`. Nothing was run or edited. I did not time `grim release` against a direct push.

## 1. Acceptance tests today

### Dependencies and lock pins (all under `test/tests/`)

| File | Covers |
|---|---|
| `test_dependencies.py` | Chain lock, diamond equal-digest merge (`:135`), conflict listing (`:151`), direct overrides edge (`:171`), cycles, caps, cross-registry edges, offline carry-forward (`:361`, `:400`) |
| `test_dependencies_add.py`, `_install.py`, `_mutations.py`, `_contract.py` | Add, install, remove and uninstall orphans, golden status fields. No range semantics. |
| `test_dependency_fixtures.py` | Self-test of `push_manifest` and `make_dependent`. |
| `test_lock_pins.py` (1646 lines) | The pin rule: moved tag keeps the pin (`:164`), remove/uninstall keeps pins, diamond tag recorded (`:887`), moved tag is no fake conflict (`:950`), real disagreement 78 (`:969`), shadowed edge (`:1034`), held target (`:1060`, `:1088`), pre-change lock migration. |
| `test_lock_edges.py` | Only the flock contention test (Unix only). Not dependency semantics. |
| `test_update_scope.py` | Named-update closure, moved pin outside the closure (`:419`), scoped prune, hook arming. The tag-moving ones use `_move_tag` (`:56`). |
| `test_held_back.py` | Bare direct `x` held at v1 while `y` requires `x:2`: warning, `status --check`. |
| `test_status_check_edges.py`, `test_status_dependencies.py` | `status --check` and `required_by` fields. |
| `test_release.py`, `test_release_requires.py`, `test_release_nonversion.py` | Real `grim release`: cascade tags (`test_release.py:114`), rolling re-release (`:149`), two-layer requires, no-cascade (`test_release_nonversion.py:111`). |
| `test_old_grim_*.py` (6 files), `test_golden_pre_dependencies.py` | 0.15.1 interop and golden bytes. |

No dedicated cascade or backport acceptance test exists. The cascade is only exercised in the release tests above.

### Tests the ADR changes or invalidates

Every edge tag in these fixtures (`./c:1`, `:1.2`, `:1.3`) is a bare-int or minor tag with no annotation. Under the ADR those are Range claims. A manifest tagged `1` has span [1,2), which fails `1.2`, and there are no full-semver tags, so the ladder falls to 79.

- **Diamond, `1` beside `1.2`:** `test_lock_pins.py:887` `test_diamond_records_the_winning_tag`. It expects `target:1@…` and holds it through a moved tag. Under the ADR the line is [1.2, 2), the evidence of `:1` fails it, and the result is 79 or a re-point.
- **Moved tag with edges `:1` then `:1.3`:** `test_lock_pins.py:950` `test_moved_tag_does_not_fake_a_conflict`. The held pin's evidence no longer satisfies the new lower bound.
- **Winner removal with holders `:1.2` and `:1.3`:** `test_lock_pins.py:287` `test_remove_of_the_diamond_winner_keeps_the_pin` (parametrized over holders). The same tag-shape evidence problem applies.
- **Bare direct yielding to a Range edge (Decision 8):** the whole of `test_held_back.py` (`:81`, `:124`, `:141`, `:162`, `:180`, `:204`, `:229`, `:281`). Its docstring (`:5`) says "The declaration wins, as it always has".
- **"Direct wins" for a bare line:** `test_dependencies.py:171` `test_direct_overrides_edge` uses a typed `c:2` direct, so D2 still holds. Re-verify only. `test_lock_pins.py:1034` `test_shadowed_edge_does_not_answer` is likewise typed and likely survives.
- **Two-major diamond:** `test_dependencies.py:151` (tags 1, 2, 3) and `test_lock_pins.py:969` still exit 78. Only the message hint may change, so re-check the asserted strings `c:{tag}` and "declare skill c directly".
- **Carry tests over a hidden conflict:** `test_lock_pins.py:324` to `:420` and `:447` (`_direct_over_two_tags`, `_direct_over_a_conflict`). They assert that `grim lock` carries where a fresh resolve exits 78. Decision 5 is a fresh-resolve 78 and `exit-agrees` stays. Re-verify only.
- **Cascade moved by `grim release`:** `test_release.py:149` asserts plain `grim update` rolls `:1` forward. It is likely unchanged, but under Q1 option (a) release would stop moving floats backward, which needs a new case.
- **No tests assert "different digests exits 78" for same-major differing minors.** `test_diamond_conflict_lists_all` and `test_real_tag_disagreement_still_exits_78` use different majors, so they survive.

Rust units that change the same way:
- `src/resolve/graph.rs:2945` `different_digests_conflict_and_the_message_says_how_to_choose`. `x:1` against `x:2` stays 78, but the message text changes.
- `graph.rs:2215`, `:2857`, `:4747`: direct-wins and "never read" assertions, to re-verify.

## 2. Publishing helpers

- **`test/src/registry.py`:**
  - `push_artifact` (`:135`) builds the manifest annotations at `:165-167`: `{"com.grimoire.kind": kind}` plus the caller's `annotations` dict, written at `:197`.
  - `push_manifest` (`:209`) writes `annotations` as given (`:241`).
  - `retag` (`:327`) GETs a manifest by digest and PUTs it under another tag.
  - `tag_digest` (`:284`) and `fetch_manifest` (`:303`) read back.
  - **There is no tag-listing helper.** The ADR's step-3 listing needs none to publish. A test that asserts on `tags/list` would need a small new GET.
- **`test/src/helpers.py`:**
  - `make_artifact` (`:341`) and `make_dependent` (`:170`) already take `annotations=`. `make_dependent` merges them at `:193`.
  - The helpers do not stamp `org.opencontainers.image.version`, but they pass it through. This needs no helper change.
- **Publish 2.3.0 with the annotation and cascade, today:**
  ```python
  a = make_artifact(repo, "skill", files, tag="2.3.0",
                    annotations={"org.opencontainers.image.version": "2.3.0"})
  for t in ("2.3", "2", "latest"): retag(repo, t, a.digest)
  ```
  For a dependent, use `make_dependent(..., annotations=...)` the same way.
- **Smallest recommended change:** one `publish_release(repo, version, files, requires=None, cascade=True)` wrapper in `helpers.py`. It adds the annotation and the `retag` loop, and its parameter names must match what the cascade computes.
  - Because the annotation is part of the manifest, each version gets a distinct digest even with identical files.
  - `_move_tag` (`test_lock_pins.py:52`, `test_update_scope.py:56`) has two traps. It retags the old digest to `keep` first, because zot drops an untagged manifest. It pushes at `tag=` with no annotation.
- **Real `grim release`:** `runner.json("release", str(src_dir), f"{repo}:2.3.0")` (see `test_release.py:114`) stamps the annotation (`src/oci/annotations.rs:420` and the other kinds) and moves the cascade. There is no shared helper for it: each test writes a local skill dir (`_local_skill`, `test_release.py:35`). A direct push is about 4 ms per repo (`helpers.py` `make_repos` docstring). A release is a subprocess plus a build, so it is slower, but I did not measure it. Use it for a few end-to-end publisher tests and direct pushes for matrices.

## 3. Templates for each scenario

- **Newer release, cascade moves:** `test_release.py:149` `test_rerelease_moves_preexisting_floating_tags`, which goes through the real release. For a direct-push version use `_move_tag` + `retag` as in `test_lock_pins.py:52`/`:950`.
- **Backport (floats move backward):** no test exists. Build it from the real release by releasing 2.9.0 and then 2.3.7. Q1 says `grim release` then points `2` and `latest` at 2.3.7. Or build it by hand with `retag(repo, "2", older.digest)`.
- **No cascade tags (full semver only):** `test_release_nonversion.py:111` `test_release_no_cascade_semver_single_tag`. The ADR's 79 case ("no full-semver and no annotation") uses plain `make_artifact` with tags like `stable`.
- **No version annotation:** every `make_artifact` and `make_dependent` fixture in `test_lock_pins.py`, `test_dependencies.py` and `test_held_back.py` already has none.
- **Offline:**
  - `test_dependencies.py:361` and `:400` use `--offline` plus `_evict_from_store` (`:356`).
  - `test_lock_pins.py:1137` is an offline lock from a carried bundle row.
  - `test_golden_pre_dependencies.py:368` `_seed_tag_cache` and `:382` seed the tag cache.
  - `GRIM_OFFLINE=1` is set through `runner.env` (`test_fix_locking.py:71`, `test_describe.py:168`).
  - A needed listing offline (exit 69) has no template.

## 4. Old-grim compatibility

- **Getting the 0.15.1 binary:** `test/src/old_grim.py:35` `checked_old_grim` reads `GRIM_OLD_BIN`. It asserts `grim 0.15.1`, then plants a lock with a later field (`transitive`) and requires 78, to reject a dev build. Modules skip without the variable (`pytestmark` in `test_old_grim_lock_pins.py:35`). `task test:old-grim` (`test/taskfile.yml:88`) runs them and fails on a skip. The deep job runs it (`.github/workflows/verify-deep.yml:353`).
- **A 0.15.1-written lock:** the `old` fixture (`test_old_grim_lock_pins.py:80-91`) runs the old binary in the same project as the current grim, each with its own `GRIM_HOME`. A committed golden fixture is also used: `test/data/golden/pre_dependencies_cdeedcaa/`, written by 0.15.1.
- **"Dependency-free lock stays byte-identical":**
  - `test_golden_pre_dependencies.py:284` `test_golden_locks_reserialize_byte_identically` byte-compares, with a named equivalence that strips `pinned` tags (`:193`, `:204`).
  - `test_release_requires.py:224` `test_dependency_free_release_byte_identical` covers the release manifest bytes.
  - It needs `GRIM_TEST_REGISTRY_HOST=localhost:5000`, and `GRIM_REQUIRE_GOLDEN=1` makes a skip a failure.

## 5. Rust harnesses

- **`Fake` in `src/resolve/graph.rs`** (tests at `:1935`, struct `:1951`):
  - `Pub` carries `annotations` (`:1992`) and `publish_members` writes them (`:2054`). Annotations are supported.
  - `list_tags` returns `Ok(None)` (`:2160`), so **tag listings are not supported**.
  - It stores one tag per `publish` and has `retag` and `untag` helpers (`:2060` area), counts resolves and manifest reads, and has offline switches (`tags_offline`, `store_empty`).
  - `OneManifestMissing` at `:4200` forwards `list_tags`.
- **`MemoryRegistry`** (`src/oci/access/memory_registry.rs`): `list_tags` (`:~85`) returns every tag of a repo, or `None` if empty. Manifests keep annotations, and `put_tag` supports several tags per digest. A real listing and annotation fake needs only a publish helper.
- **`src/resolve/agreement_tests.rs`:** the online/offline graph-walk parity test. Its `publish` (`:~40`) has annotations `content` and the kind only.
- **`src/resolve/pin_agreement_tests.rs`** (6321 lines, wrapped in the `resolve.rs` `mod`):
  - The `Store` wraps `MemoryRegistry` (`:60`) and delegates `list_tags` (`:112`).
  - `Art` and `Edge` (`:140-160`) have no version field, so annotations cannot carry a version yet.
  - The sweep is `every_writer_holds_the_pin_rule` (`:4153`).
- **`relock-error`:** defined at `:3147`, `(None, _) => fail("relock-error", …)`. The judge `conflict_key` (`:3571`) returns a key only for `ResolveErrorKind::BundleConflict`. Any other error fails the clause. ADR text: `adr_lock_pin_stability.md:289`. A new exception widens `conflict_key`, or adds an arm before the `(None, _)` fail, to cover `UnchosenRequirement` and `MajorDowngrade` (78), 79 `NoMatchingVersion` and the two 65 budgets.
- **`exit-agrees`:** defined at `:3157`, `:3167` and `:3184`. ADR text: `adr_lock_pin_stability.md:271`. It asserts a relock refuses only what a fresh resolve refuses and accepts nothing a fresh resolve refuses. Existing exceptions are guards inside the same `match`:
  - `world.moved && named_any(...)` (`:3157`)
  - `typed && gained_an_edge(...)` (`:3167`)
  - `carried_through_a_moved_tag` and the carried sets (`:3184`)

  A new allowed exception is one more guard arm on the `(Some(key), None)` branch, and on the accepted-where-fresh-refuses branch at `:3178-3190`. Name it, for example, `MajorDowngrade` or the pin form of `UnchosenRequirement`.
- ADR line `.agents/adr/adr_dependency_version_claims.md:280` confirms this harness plan, plus "`--allow-downgrade` joins the writers".

## 6. Suite mechanics

- **Running:** `task test` runs `uv run pytest -m "not index_e2e and not bench"` under `ocx exec` (`test/taskfile.yml:29`). `test:parallel` and `test:quick` add `-n auto` (xdist). `test:shard` runs `tools/accept_shard.py` modules `[I::N]` (modules sorted by stem).
- **Registry:** a session fixture starts a private zot on a random loopback port unless `GRIM_TEST_REGISTRY_HOST` names one.
- **Floor and ceilings:** the numbers are in `test/SUITE_FLOOR` (**3456**), `test/SKIP_CEILING` (**193**) and `test/XFAIL_CEILING` (**0**). Commit `4858c79b` raised them from 3385 and 188 for 71 added cases (5 skip without `GRIM_OLD_BIN`, plus 4 extra macOS skips).
  - They are enforced by `test/tools/accept_floor.py:108-115`, in PR CI through `task test:floor`.
  - An unfiltered local `task test:parallel` only warns, through `test:floor:warn` (`taskfile.yml:168`).
  - **Raise `SUITE_FLOOR`** by the number of cases added, so it does not hide a lost test. **Raise `SKIP_CEILING`** only for new skips: the new `GRIM_OLD_BIN` cases and any Windows-only skips. Check the pytest parametrize counts, because the floor counts cases, not functions.
  - Removed or merged tests lower the count and can break the floor. This matters because several tests above must be rewritten.
  - A new `test_*.py` file is picked up automatically by sharding.
- **Windows:** `@pytest.mark.skipif(sys.platform == "win32", reason=…)` or `os.name == "nt"` (`test_lock_edges.py:26`, `test_update_scope.py:800`). Windows runs unsharded and outside the floor (`verify-deep.yml:164-170`). Dependency tests should not need Windows skips.
- **Runtime:** no per-test timeout in `pyproject.toml`. Tests are subprocess-light. Resolve scenarios with several `grim` runs are the slow ones.
- **Style (`quality-python.md`):** typed, no bare `except`, no `assert` for runtime validation outside tests, and per `subsystem-tests.md` every text read or write names its codec and newline (`test_text_io_hygiene.py` fails on a bare `read_text`/`write_text`).

## 7. Narrowest commands

- **Rust resolver units:** `cargo nextest run resolve::` (or `cargo test resolve::`). The agreement sweep alone is `cargo test resolve::pin_agreement_tests::every_writer_holds_the_pin_rule` (about 75-110 s per `.claude/state/lock-pin-stability-review7/.../results.tsv`). The graph units alone: `cargo test resolve::graph::tests`.
- **Acceptance:**
  ```
  cd test && uv run pytest tests/test_dependencies.py tests/test_lock_pins.py tests/test_held_back.py tests/test_release.py tests/test_release_requires.py -n auto
  ```
  Add `tests/test_old_grim_lock_pins.py` with `GRIM_OLD_BIN` set. The binary must already be at `test/bin/grim`, or set `GRIM_COMMAND`.

## Existing tests the ADR invalidates (must change or be re-pointed)

1. `test/tests/test_held_back.py`: all of it (Decision 8, bare direct yields to a same-repository Range edge).
2. `test/tests/test_lock_pins.py::test_diamond_records_the_winning_tag` (`:887`).
3. `test/tests/test_lock_pins.py::test_moved_tag_does_not_fake_a_conflict` (`:950`).
4. `test/tests/test_lock_pins.py::test_remove_of_the_diamond_winner_keeps_the_pin` (`:287`).
5. Re-verify only, likely survivors:
   - `test_dependencies.py:135`, `:151`, `:171`
   - `test_lock_pins.py:324-420`, `:447`, `:969`, `:1034`
   - `test_status_check_edges.py`
   - `test_release.py:149`
6. Rust: `graph.rs:2945` (message text), `:2215`, `:2857`, `:4747`. In `pin_agreement_tests.rs`, `conflict_key` (`:3571`) and the `exit-agrees` and `relock-error` arms (`:3147-3190`), plus `adr_lock_pin_stability.md:271` and `:289`.

## Fixture and helper changes needed before any new acceptance test

1. **A `publish_release` helper** in `test/src/helpers.py`: the version annotation, the cascade `retag` loop, and an optional requires layer. No changes to `registry.py` are strictly required, since `annotations=` and `retag` already work.
2. **A `list_tags(repo)` GET helper** in `test/src/registry.py`, only if a test asserts on the listing.
3. **A backport and annotation-lie fixture:** one version retagged under a lower cascade tag, or an annotation that contradicts its tag.
4. **A real-`grim release` helper** shared across tests (today each test writes its own local skill dir).
5. **Rust `Fake` in `graph.rs`:** a real `list_tags` (`:2160` returns `None`), multi-tag publish, and annotation support in the shared `Pub` helpers.
6. **`pin_agreement_tests.rs`:** a version field on `Art` so it can set the annotation, the `--allow-downgrade` writer, and widened `conflict_key`.
7. **Raise `test/SUITE_FLOOR` and `SKIP_CEILING`** by the cases added.

Not checked: whether `grim release` stamps the annotation for skills and rules (I only confirmed `annotations.rs` stamps it per kind, and `test_settings_publish.py:134` asserts it for settings), and how slow a release is against a direct push.

---

# Part: CLI, status and errors

Read-only recon of the CLI surface for `--allow-downgrade`. I read the ADR (flag, errors and reports sections) and left the resolver internals alone. Nothing was edited and no gate was run.

## 1. Flag plumbing

**`HookTrustOpts` pattern**
- Definition: `src/cli/options.rs:146` is a `#[derive(Debug, Clone, Copy, Args)]` struct of two `bool`s, with `impl` accessor `flag()` at `:172`. Unit tests sit at `:195+`.
- Flatten sites, each with `#[command(flatten)] pub hook_trust`:
  - `src/command/add.rs:106`
  - `src/command/update.rs:109`
  - `src/command/install.rs:68`
- How commands read it: `args.hook_trust.flag()` at `add.rs:242`, `:669`, `:1265`, `:1366`; `update.rs:162`; `install.rs:98`, `:390`.
- Test constructors build the struct as a literal: `add.rs:3329` and `update.rs:2468`.
- `DowngradeOpts` should copy this shape.
  - A `bool` plus an accessor returning the `Downgrade` enum fits the ADR's "enum, not a `bool` parameter".
  - It needs a clap parse test next to `TrustHarness` in the `options.rs` tests.

**`LockArgs`** is `pub struct LockArgs {}` at `src/command/lock.rs:44`. `run` takes it as `_args` (`:51`).
- The struct gains the flattened opts, and `_args` becomes `args`.
- Test constructors `LockArgs {}` must change at:
  - `lock.rs:436`, `:440`, `:450`, `:468`, `:472`
  - `update.rs:2859`
  - `add.rs:3631`
- Struct-literal constructors of `UpdateArgs` (`update.rs:2463`, `:2476`, `:2607`, `:2671`) and `AddArgs` (`add.rs:3324`) also need the new field.

**Conflicts**
- `update --marketplace`: `conflicts_with_all = ["force", "client"]` at `update.rs:~100`. `--global` and `--config` are refused at runtime (`update.rs:1118`).
- `--allow-downgrade` conflicts with none of these, since `--marketplace` does resolve. `hook_trust` is also not in that conflict list.
- `lock` and `add` have no other conflicting flags.

**Places a flag is mirrored**
- Clap derive is the single source. There is no snapshot or help-text test.
- `src/command/completions.rs:44` generates completions from `Cli::command()`, so nothing to regenerate. `test/tests/test_completions.py` has no per-flag pins.
- `DescribeCliReport` (`src/api/describe_report.rs`) describes artifacts, not CLI flags, so it is irrelevant.
- No published schema lists flags.
- Hand-written docs to edit:
  - `docs/src/content/docs/commands.md`: `add` synopsis `:323`, `lock` `:610` (`#lock-pins` `:636`, exit codes `:722`), `update` `:909` (exit codes `:1163`, `--marketplace` `:1180`). It also needs the new 78 rows and a new `--allow-downgrade` description.
  - Catalog skills that spell out flags and error reasons: `catalog/skills/grim-usage/references/updating.md`, `consume.md`, and `troubleshooting.md:57` (the reason list).
- `docs/dist/*` is generated output (`task docs:build`). Do not edit it.
- Not on `install`, per the ADR. `src/main.rs:103` is the `Lock(LockArgs)` dispatch, unchanged.

## 2. `ResolveOptions` and every `target_lock` caller

`ResolveOptions` (`src/resolve/resolve_options.rs:20`) has `per_artifact_timeout`, `max_retries`, `base_backoff`, and a `Default` impl. A new `downgrade` field needs a `Default` value (`Refuse`).
- Struct literals that need the field (or `..Default::default()`):
  - `resolve/agreement_tests.rs:176`
  - `resolve/graph.rs:2186`, `:3687`
  - `resolve/resolver.rs:1531`, `:1720`
  - `resolve/pin_agreement_tests.rs:1471`
- The `resolve_options.rs` default test also needs an extra assertion.
- Internal consumers take `options` already: `members.rs:87`, `:110`, `part_bundles.rs:62`, `held_back_warnings` (`resolver.rs:250`).

Production callers. Each passes `ResolveOptions::default()` today, and the error surfaces as follows:

| Caller | Site | Error display |
|---|---|---|
| `add` | `src/command/add.rs:371`, inside `write_config_and_relock` (`:348`) | `super::grim(target)?` at `:384`. The config is rolled back (`:399`). Exit is classified, and the text goes to stderr through `printed_chain`. |
| `add` callers | `:238` (run) and `:665` (path source) | The function takes `trust` and `offline` positionally, so a `Downgrade` parameter goes in both. |
| `update` | `update.rs:141` | `super::grim(...)?` at `:140`. |
| `lock` | `lock.rs:67` | `super::grim(...)?` at `:66`. |
| `update --marketplace` | `update.rs:1151`, via `resolve::resolve_marketplace(...)` (`export/resolve.rs:347`) | `.await?`. It builds `ResolveOptions::default()` internally at `export/resolve.rs:440`. |
| Export stage | `export/resolve.rs:435` | A `.map_err` ladder at `:455-500` rewrites kinds per seam. |

- Callers of `resolve_marketplace`:
  - `update.rs:1151`
  - `export/stage.rs:256`, `:440`
  - `export/resolve.rs:985`, `:1858`, `:1899`, `:2087`, `:3740` (tests)
  - `resolve/pin_agreement_tests.rs:2806`
  - A `Downgrade` parameter or options argument ripples through all of these.
- In the export `map_err` ladder, `HeldRequirement` (`:468`) fills `retry` with `update_command(&m.path, plugin)` (`export/resolve.rs:589`, which prints `` `grim update --marketplace <path> <plugin>` ``).
  - `MajorDowngrade` needs a similar arm. Export passes `Refuse`, so the message must name `grim update --marketplace … --allow-downgrade`.
  - `BundleConflict` becoming `PluginConflict` (`:482`) is the precedent for per-seam rewording.

TUI. There are two production `target_lock` sites, not four. `ResolveOptions::default()` appears at four production-ish lines:

| Site | Line | Purpose |
|---|---|---|
| Bundle member fetch | `src/tui/app.rs:619` | Options only; passed to `bundle_checker.spawn_fetch(.., &options)` (`tui/bundle_member_fetch.rs:155`). It is not a `target_lock` call. |
| `held_by` | `:3548-3549` | Catalog-row probe. Only `BundleConflict` is absorbed (`conflicts_on`, `:3567`); other errors propagate. |
| `relock_row` | `:4081-4083` | The shared relock for Install and Update. Called from `:3385` and `:4252`. |
| Tests | `:8997`, `:9406` | Test helpers; they need `Refuse` only if the signature changes. |

- A TUI error reaches the user as an `anyhow` error through `absorb_failure` (`:4619`) and the batch failure path.
  - The refusal text must name the CLI command itself. For example, `ResolveError.grim` (`Invocation`, via `.naming(grim)` and `LockRun.grim`) carries `grim`, `grim --global` or `grim --config <path>`.
  - The existing precedent is `gone_pin_remedy(&self.grim, name)` (`resolve_error.rs:168`). `refuse_overwrite` (`tui/app.rs:~4100`) also embeds `` `{grim} add --name …` ``.
  - `{fix}` is per command, per the ADR. The TUI and export need their own fix strings.

## 3. Errors, end to end

**Variant location.**
- `ResolveErrorKind` is `#[non_exhaustive]` and uses `thiserror` (`src/resolve/resolve_error.rs:250`).
- Latest template: `HeldRequirement` (`:382`) is a struct variant with an `#[error(...)]` template that wraps fields in `escape_controls`. It also has a Display test at `:574`.
- `ResolveError::Display` (`:177`) already appends `; required by {parent}`.
- The `Display` for `MajorDowngrade` needs the per-command `{fix}`, as `retry` does in `HeldRequirement`.

**Exit mapping.**
- `classify_resolve` (`src/error.rs:585`) is exhaustive, with no wildcard, so a new kind will not compile until an arm exists.
- Today `BundleConflict`, `PluginConflict` and `HeldRequirement` map to `ConfigError` (78) with no reason at `:609-611`.
- The ADR wants these:
  - 78 with no reason: `UnchosenRequirement`
  - 79: `NoMatchingVersion` (joins the `NotFound` arm, `:587`)
  - 65: `GraphTooLarge` variants (already in the `DataError` arm, `:605`)
  - 78 with a reason: `MajorDowngrade`
- `MajorDowngrade` would use a block like `StaleLock` (`:597`):
  ```rust
  Classification { exit: ExitCode::ConfigError, reason: Some(ErrorReason::MajorDowngrade) }
  ```

**Reason slug.**
- `ErrorReason` enum is at `src/error.rs:177`.
  - The new variant is appended last, after `UnsupportedArtifactFeature`.
  - Its `Display` match is at `:216-229`, and it needs `"major-downgrade"`.
- `retryable` (`:232`) and `forceable` (`:244`) match on the variant. The ADR says both are false, so the default arms suffice.
- The JSON shape is built in `src/main.rs:301` (`error_document`). It emits `reason`, and emits `retryable` or `forceable` only when true.

**Tests to extend.** In `src/error.rs`:
- `error_reason_slugs_are_locked` (`:1032`)
- `retryable_*` and `forceable_*` (about `:1050-1090`)
- `FROZEN_REASON_SLUGS` (`:1778`) plus its position match and `every` array (`:1862-1889`)
- A new classification test following `unsupported_artifact_feature_is_65_with_its_reason_everywhere` (`:1892`)
- Update `a_plugin_conflict_and_a_held_requirement_classify_as_config_errors` (`:~810`) if new 78 kinds are added.
- Also extend `src/command/export.rs:674-712`, where `literal()` and the iteration list are exhaustive over `ErrorReason`.
- Add a Python acceptance case in `test/tests/test_json_interface.py`, patterned on `test_error_reason_marks_no_lock` (added in commit `91d80421`).

**Docs.**
- Add a `major-downgrade` row (`data` or config 78) to the reasons table at `docs/src/content/docs/json-interface.md:~1078-1088`. Copy the `no-lock` row.
- Reasons are not in any published schema. `docs/public/schemas/*` has no reason list, and `grim-publish` and `grim-settings` only contain "reason" prose.
- Update `catalog/skills/grim-usage/references/troubleshooting.md:57`.
- Optionally add to `stability.md`.

**Template to copy.**
- Slug template: `91d80421` (`NoLock`). It touches the enum, `Display`, the frozen slug test, `export.rs` `literal`, the json-interface docs row, and the Python test.
- Resolve-kind template: `HeldRequirement` (a kind, an exit arm, and a per-seam retry rewrite in `export/resolve.rs:468`).

## 4. `grim status --check`

- Row building: `src/command/status.rs:641` calls `row_references(l, &member_key, declared)`, which pushes one `UpdateCheck` per reference.
  - The `declared` identifier comes from `member_declared_id`, or `dependency_reference` (`:2611`) for a dependency-only row.
  - `dependency_reference` uses `recorded_reference`: the pin's own tag if an edge names it, else the smallest edge.
- `row_references` (`:959`) returns the declared identifier, or every recorded edge when the key has more than one recorded reference.
- `fresh_digests` (`:1187`) resolves each reference through `resolve_declared_digest` (`catalog/update_availability`). This is a tag-to-digest lookup, not claim-aware.
- `row_answer` (`:984`):
  - Any failed or capped reference gives `Read(None)`.
  - No resolved digest gives `Read(Some(false))`; a vanished tag is never newer.
  - Resolved digests that differ give `Split`.
  - Some resolved and some vanished gives `Read(None)`.
  - Otherwise `Read(Some(outdated_from_resolve(locked, fresh)))`, so `update_available` is a digest-inequality test.
- A "split" is two or more recorded references for one key that resolve to different digests. The row reads `update_available: null` and `split_note` (`:1103`) prints one stderr line: `… name different digests; \`{grim} update\` exits 78 until grimoire.toml declares one`.
  - The sweep driver is `:859-868`; `notes.extend(split)` is at `:874`.
- The ADR change: for a dependency row whose recorded edges are same-repository Range claims, resolve `T(line)` of each claim and compare with the pin, so such rows stop reading as a split.
  - The relevant code is `row_references` plus `row_answer`, and likely a new claim-aware resolve in place of the plain `resolve_declared_digest` for these checks.
  - Direct rows and bundle members stay unchanged.
  - An empty line still yields a split.

JSON fields involved:
- `update_available` (`true`/`false`/`null`)
- `checked`
- `moved_nested_bundles`

**Tests**
- Rust unit tests in `status.rs`:
  - `a_key_whose_references_name_different_digests_reads_null` (`:3498`)
  - `a_key_whose_references_agree_answers_for_its_pin` (`:3534`)
  - `a_row_the_cap_cuts_short_reads_null` (`:3561`)
  - `a_dependency_row_answers_only_on_edges_the_store_lists` (`:4454`)
  - the diamond helpers `diamond_lock`, `diamond_checks`, `diamond_registry` (`:~3440-3495`)
- Python: `test/tests/test_status_check_edges.py`, with `test_status_check_answers_null_where_a_keys_references_name_different_digests` at `:72` parametrized on `b1-first`, `b2-first` and `requires`. The `requires` shape is where the behavior changes.

**Docs**
- `docs/src/content/docs/commands.md:1430` (`#status-check`), especially `:1480-1500` (a dependency row answers by tag; "A key the lock records under more than one reference…").
- `docs/src/content/docs/json-interface.md:~505-520` (the nullability table; the row "references resolve to different digests…" at `:517`).
- `docs/src/content/docs/stability.md:217` and `:315`.

## 5. Reports and stderr lines

**JSON shapes stay unchanged.**
- `LockEntry` in `src/api/lock_report.rs:~30` carries `disposition`, whose `moved` serialization is at `:44`.
- `AddReport` in `src/api/add_report.rs` has `moved: Vec<PinnedNode>` (`:63`, `with_moved` `:105`). Its `notes` line `moved N pin(s): …` is at `:132`.
- `UpdateReport` has `src/api/update_report.rs`; `moved` there refers to the `updated` rows.
- Report producers:
  - `lock.rs:build_report` (`:~162`)
  - `add.rs`: `moved_pins` (`:722`), `dropped_pins` (`:744`)
  - `update.rs`: the report is built from dispositions.

**Stderr lines today.**

| Function | Location | Emits |
|---|---|---|
| `warn_moved_bundle_pins` | `src/command/lock.rs:94`, called from `lock.rs:~87`, `add.rs:~398`, `update.rs:~171` | `tracing::warn!("bundle '{}' moved its pin from {} to {}", …)` at `:120` |
| `NamedScope::warn_left_behind` | `src/command/update.rs:742` | the "also changed … outside the named closure" line |
| `warn_relock` | `src/resolve/resolver.rs:660`, called from `target_lock` `:238` | tagless, unvouched, offline and part warnings, in a `[...]` array `:695-705` |
| `held_back_warnings` | `resolver.rs:250` | `GraphWarning::HeldBack` (`graph.rs:146`), printed at `:187-189` |
| `kept_pin_note` | `add.rs:766` | note when `add` kept a pin |
| `warn_outside_closure` | `src/tui/app.rs:3721` | the TUI calling `warn_left_behind` |

**Where the new lines go.**
- "Moved because a dependency requires X", "yielded to a dependency", and "narrowed by a dropped version" are resolve-time facts. Natural homes:
  - `GraphWarning` (`graph.rs:146`), a new variant with a `Display` arm, printed by the loop at `resolver.rs:187`. This is the held-back pattern, and it covers add, update, lock and the TUI uniformly.
  - Alternatively, add an entry to the `warnings` array in `warn_relock`, if the data comes from the dispositions.
- Do not add them in each command. `warn_moved_bundle_pins` is the only command-level hook, and it is bundle-specific.
- Add Rust tests in the `resolver.rs` tests, near `held_back_warnings` (`:4192-4398`).

## 6. Rules, docs and gates

**`.claude/rules/subsystem-cli-commands.md`.** The relevant rows are 21, 22, 26 and 34 in the command table, and each is a single long line.
- `:21` `grim add`: the synopsis lists `[--force] [--[no-]trust-workspace]`, so it needs `--allow-downgrade`.
  - Stale text: "the target resolves unless the lock holds a pin that answers its reference"; "Plain output prints one stderr line naming each pin the run moved"; the stderr note list (the new "moved because" lines are not mentioned); `kept_pin_note` on tag equality.
- `:22` `grim lock`: it states "**without moving a pin**: `Refresh::Nothing`… a node resolves only if it is new or its request no longer matches the pin", and `lock::pin_rule` owns what a pin answers.
  - That is stale for a below-lower-bound claim, which now moves with no flag, and for a major downgrade, which needs `--allow-downgrade`.
  - No flag is mentioned.
- `:26` `grim status`: "`dependency_reference`… `row_references`… `split_note`" are described exactly; the Range-claim rows no longer split.
- `:34` `grim update`: "`--marketplace` conflicts with `--client`/`--force`"; "The command that moves a pin whose request held", with the held-back warning described; no flag listed.
- `subsystem-cli.md` (the output-layer, `Command Pattern` and TUI popups text) states nothing that goes stale. At most it needs nothing.
- `subsystem-cli-api.md` is mostly unaffected: JSON shapes do not change. The error-document paragraph names `reason`, `retryable`, `forceable` and `hint` generically, which stays true.
- Other docs that go stale: `concepts.md` ("a direct declaration always wins", per ADR Decision 8) and `upgrading.md`.

**Gates**
- `task claude:tests` runs `.claude/tests/test_ai_config.py` and `test_upstream_stale.py` (structural: catalog coverage, references, link check).
  - It does not diff the rule prose against code. Prose drift is caught by review, and `meta-validate-context` covers it.
- `task catalog:verify` runs `grim build` on every catalog package (`catalog/taskfile.yml`).
  - It validates catalog packages against the real parser, not flag mentions.
  - `test/tests/test_catalog_drift.py` feeds the skill's reference forms to the real binary. It is not a flag-list gate.
- `task docs:check` (`taskfiles/docs.taskfile.yml:69`) runs `docs/check_urls.py`, the doc-declaration check, the theme and landing checks, and the strict build.
  - Anchor and link breakage fails there, so keep the existing `{#…}` ids.
- Rust: `task rust:verify`. The `error.rs` tests lock slugs; `test/tests/test_json_interface.py` covers the error document.

## Files this surface work would edit

**(a) Flag plumbing**
- `src/cli/options.rs` (new `DowngradeOpts`, `Downgrade` enum, tests)
- `src/command/add.rs`, `update.rs`, `lock.rs`
- `src/resolve/resolve_options.rs` (the new field and default test)
- Test-literal fixes: `src/resolve/agreement_tests.rs`, `src/resolve/pin_agreement_tests.rs`, `src/resolve/graph.rs`, `src/resolve/resolver.rs`
- `src/export/resolve.rs`, `src/export/stage.rs` (signature ripple through `resolve_marketplace`)
- `src/tui/app.rs` (both `target_lock` sites, plus test helpers)
- Docs: `docs/src/content/docs/commands.md`
- Catalog: `catalog/skills/grim-usage/references/updating.md`, `consume.md`
- Rules: `.claude/rules/subsystem-cli-commands.md`

**(b) Errors**
- `src/resolve/resolve_error.rs` (`MajorDowngrade`, `UnchosenRequirement`, `NoMatchingVersion`, `GraphTooLarge` variants)
- `src/error.rs` (`classify_resolve`, `ErrorReason`, `Display`, frozen slug tests)
- `src/command/export.rs` (`literal` match, `:674-712`)
- `src/export/resolve.rs` (a `map_err` arm that names `grim update --marketplace … --allow-downgrade`)
- `src/tui/app.rs` (refusal text naming the CLI command)
- Docs: `docs/src/content/docs/json-interface.md`, `catalog/skills/grim-usage/references/troubleshooting.md`, optionally `stability.md`
- Tests: `test/tests/test_json_interface.py` (and `test_update.py` or `test_lock.py` for the 78 paths)

**(c) `status --check`**
- `src/command/status.rs`
- Docs: `docs/src/content/docs/commands.md` (`#status-check`), `json-interface.md` (nullability table), `stability.md`
- Tests: `test/tests/test_status_check_edges.py`
- Rules: `.claude/rules/subsystem-cli-commands.md` row 26

**(d) Reports and stderr lines**
- `src/resolve/graph.rs` (a new `GraphWarning` variant and `Display`)
- `src/resolve/resolver.rs` (the print loop `:187` or `warn_relock`)
- Only if lock, add or update need new fields: `src/api/lock_report.rs`, `add_report.rs`, `update_report.rs`. The ADR says shapes stay unchanged, so likely none.
- Docs: `commands.md` (lock held-back section `#lock-held-back`), `upgrading.md`

**Gaps and risks**
- I did not read the ADR's open questions (Q1, Q2) or its worked examples. Behavior there may change the status and stderr details.
- The "four TUI sites" are two `target_lock` calls (`app.rs:3549`, `:4083`), one options-only site (`:619`), and test sites. That is not four distinct production `target_lock` callers.
- I did not run any gate, so I cannot confirm that `task claude:tests` or `task docs:check` pass at baseline.

---

# Part: Docs, catalog, help text and rules

The ADR's text needs edits in seven docs pages, the three grim catalog skills, four help strings and four rule files. A dependencies page already exists, so the claim grammar does not need a new page. I read all of it, including the Principle 9 table and the open questions Q1–Q3. Q1 and Q2 change a few of the wording choices below, so I mark them. I edited nothing.

## 1. Docs pages that go wrong or incomplete

All paths below are under `/home/mherwig/.herdr/worktrees/grimoire/uno/docs/src/content/docs/`.

**`concepts.md`** (`doc_type: explanation`)
- **Lines 192-199**, the conflict policy: "a **direct** … declaration always wins over every bundle and dependency". This is now true only for a typed tag or a digest. A bare direct line yields to a same-repository `requires` major claim (Decision 8, rows D2–D4).
- **Lines 197-199**: "Two claims agree when their references resolve to the same manifest digest, so `:1` and `:1.2` agree while they point at one digest". For `requires` edges, claims in one major now settle on the newest in that major. Equal digest remains the rule for bundle members and non-semver tags (Decision 2, rows E3/E4).
- **Lines 200-202 and 207-211**: "claims that **disagree** fail closed". Add that two majors on one node exit 78 whatever the flag, and that moving to a version below the held pin's major exits 78 `major-downgrade` unless `--allow-downgrade` is given.
- **Lines 213-219**, the held-back warning: it must also name the held-back case that now yields, and a held pin that still satisfies every claim.
- **Lines 225-239**, "Floating versus pinned members": add that a bundle-member tag stays literal. A `requires` tag becomes a claim.
- **Lines 277-280**, "Floating by default. A dependency written `./gws-shared:1` is a floating tag": this is the new grammar home. Add the table (`2.1` means [2.1.0, 3.0.0), `0.2` means [0.2.0, 0.3.0), no tag or `latest` means any version, `2.x`/`v2`/`stable` are literal), "newest in the claimed major", the sticky held pin, and the stderr lines.
- **Lines 287-289**, "Your direct declaration wins": narrow it to match Decision 8.
- **Lines 291-312**: add the two new limits, a work budget of 32,768 node admissions (exit 65) and 32 MiB of tag listings (exit 65), to the limits table. The 78 and 79 refusals go in the Conflict policy subsection (item 1), since the 65 limits belong in the table.
- **Lines 314-345**, offline: a needed tag listing is always an offline miss (69). A store miss keeps the held pin with one warning.
- **Lines 102-106**, "A **floating tag** such as `:1` points at the newest `1.x` release": still correct. Leave it.

**`commands.md`** (`doc_type: reference`)
- **Line 638**, "the same registry and repository, the same tag, and the same digest": this changes only if Q2 resolves as (a), where a bare line is answered by any tagged pin in its repository. For `requires` Range claims, a pin now stays when its evidence satisfies the claim even if the tag differs.
- **Lines 458-463**, `add`: "If the claims still disagree, `add` exits `78`". Add the major-downgrade refusal and `--allow-downgrade`.
- **Lines 405-407 and 698-720**, held-back sections: add the yield rule.
- **Line 593**, `add` exit-code table (rows 598-603): the 78 row "Sources claiming different versions…" needs two majors and `major-downgrade`. The 79 row needs "no release satisfies the claim". The 65 row needs the two new budgets.
- **Lines 722-745**, `lock` exit-code table: same additions. It needs a `--allow-downgrade` description. `LockArgs` has no flags today (`src/command/lock.rs:42`).
- **Lines 1165-1174**, `update` exit-code table: same additions. `update` needs `--allow-downgrade`, including with `--marketplace`.
- **Line 911**, "re-resolves floating tags": add that `update` moves a claim-chosen pin to the newest in its claimed major.
- **Line 1340-1342**, "a direct declaration wins over a dependency", and **lines 1686-1691**, the `grim why` `overrides:` line: both need the yield caveat. A bare line may now yield.
- **Lines 1482-1498**, `status --check`: "A dependency row answers … by the tag its pin carries … a diamond that diverges, is not predicted". For rows whose edges are Range claims of one repository, it now resolves `T(line)` and compares with the pin. Such rows no longer read as a split (`split_note`), though an empty line still does. A pin recorded from a listing (`x:2.4.1@…`) sees a newer release only through the cascade tag. The same wording sits at line 1469 and in `json-interface.md:489-494`.
- **Lines 4012-4034 and 4085-4102**, `release` and `publish`: "`1.2.3`, `1.2`, `1`, and `latest` are all moved". Add that a float is never moved to a lower version than it names. This costs one manifest read per float. It depends on Q1 option (a). Also check whether `publish` shares `move_tags` (`src/command/release.rs:822`).

**`json-interface.md`** (`doc_type: reference`)
- **Line 1044**, `config` | 78: add "conflicting version claims (two majors, a dropped-version claim, a major downgrade)".
- **Line 1045**, `not-found` | 79: add "no release satisfies a claim".
- **Line 1039**, `data` | 65: add the work and listing budgets.
- **Lines 1078-1088**, the `reason` table: append a `major-downgrade` | `config` / 78 row. It is not forceable and not retryable. The other new refusals carry no slug.
- **Lines 1136-1142**, the `forceable` paragraph: "Today `modified` and `untracked-destination` set it". Add that `major-downgrade` is not forceable and its remedy is `--allow-downgrade`. VS Code keys on the slug.
- **Lines 489-494**, `status --check` dependency row: same change as `commands.md:1482`.
- **Lines 172-177**, `lock` rows: say that `moved` covers a pin that moved because a lower bound rose.

**`stability.md`** (`doc_type: explanation`)
- **Line 32**, frozen CLI surface (flags and exit codes), and **line 42**, frozen "release/push mechanics": the new flag, the exit reuse, and the release float guard all fall under these. Record them.
- **Lines 131-147**, the additive history: it mentions "the error document gained the reason `unsupported-artifact-feature`". Add `major-downgrade` and `--allow-downgrade` there.
- **Line 225**, "Eleven changes are not additive": this is the non-additive list, and the count is 11. The `release` float guard and the dropped-diamond 78 are behaviour changes, so add them as items (and bump the count) if you count them as non-additive.
- **Lines 313-315**, "`grim lock` and `grim add` exit `0` with the pin kept on a diamond… Bare `grim update` still exits `78`": add that `B:2` beside `B:2.1` is no longer a diamond, since it resolves.
- **Lines 209-212**, `update_available` for dependency rows: add the `T(line)` answer.

**`upgrading.md`** (`doc_type: troubleshooting`)
- It already has the 0.16.0 sections at **459** ("Declared dependencies need grim 0.16.0"), **521** ("Install fixes in 0.16.0"), **625** ("`grim lock` keeps pins") and **1154** ("Smaller notes"). The ADR's Principle 9 row maps to the last row of the table at **474-481**.
- Add a section, or extend 459-519, covering these items:
  - `requires` diamonds within one major now resolve.
  - A bare direct line yields to a `requires` major claim.
  - A moved pin may record `x:2@…` where a requester wrote `2.1`.
  - `--allow-downgrade` on `add`, `update` and `lock`, and reason `major-downgrade`.
  - Two majors on one node exit 78.
- Add a separate item for `grim release`: a float (`2`, `2.1`, `latest`) is not moved to a lower version. This one affects already-published artifacts, so put it where a publisher will look (**line 1154**, Smaller notes, or a new section).
- Lines **625-720** ("What changes for a moved tag"): add a sentence that pins within a claim line stay.

**`artifacts.md`** (`doc_type: reference`) — this is the declaration home
- **Lines 683-688**, the requires reference grammar ("The reference is a fully-qualified `registry/repo:tag`…"): add a subsection, "What the tag claims", with the claim table. This is where authors learn that `./gws-shared:1` means >=1.0.0 and <2.0.0. State that bundle `[skills]` members keep literal tags.
- **Lines 574-577**, bundle members: "Floating tags re-resolve on `grim update`". Add that these stay literal.
- **Lines 727-732**, `grim build` checks: say that no claim check happens offline.

**`publishing.md`** (`doc_type: how-to`, `doc_tier: integration`)
- **Lines 694-707**, "Cascade tags": add "never moves a float to a lower version". Use the backport example, 2.9.0 then 2.3.7: `2` and `latest` stay on 2.9.0. Without the guard, `2` and `latest` would move back to 2.3.7. Note that the release now does a manifest read per float, and that already-published and third-party artifacts are not fixed.
- **Lines 987-996**, "Who can install it": "Dependencies stay as shared as the tags they name… can therefore conflict". Replace it. Tags in one major no longer conflict.
- **Lines 961-970**, publish order: "declare `./gws-shared:1.0.0` and not a floating tag if you rely on the ordering". This still holds (the ordering uses the exact sibling tag). `1.0.0` is now also a claim of >=1.0.0 and <2.0.0, so add a clause.
- **Lines 956-959**, `--pin`: say it freezes a claim tag to a digest claim. A digest claim is exact.

**`configuration.md`** (`doc_type: reference`)
- **Lines 235-241**: "a direct declaration wins over any bundle or dependency, agreeing claims coalesce, and disagreeing ones fail closed". Narrow it as in `concepts.md`.
- **Lines 907-913**: "A dependency records the request that won, resolved against its parent's pin". A moved or new pin may record a cascade tag (`x:2`) where the requesters wrote `2.1`. A committed lock keeps it, so this is the one-way recorded tag.
- **Lines 951-1008**: the `[[<kind>.requires]]` `id` is still stored as written. No schema change.

**`guides/dependencies.md`** (`doc_type: how-to`, `doc_tier: everyday`) — exists, 295 lines, titled "Bundle or require"
- **Lines 262-270**, "When the chain is not what you expected": "A direct declaration beats whatever an artifact asked for". Add the bare-line yield and a worked two-requesters example.
- **Lines 69-71, 83, 214**: the `./templates:1` and `"./gdrive:1"` examples are fine and claim >=1.0.0 and <2.0.0. Add one sentence saying so, in this guide or via a link.
- It is exercised by `test/tests/test_guide_dependencies.py` (blocks marked `<!-- doc: guide-dependencies -->`). Any `bash-run` change must still pass that harness. A prose-only addition is safe.

**`guides/versioning.md`** (`doc_type: how-to`, `doc_tier: everyday`)
- **Lines 12-27**, the five-rung ladder: it describes direct declarations only. Add a note that in a `requires` value the tag is a claim.
- **Lines 69-72**: "A release of 1.2.3 points `1.2`, `1` and `latest` at that digest as well." Add the no-lower-version guard.

**`changelog.md`** (`doc_type: changelog`): no change now. See section 7.

No change needed in `quickstart.md` (55, 119) or `guides/team-ci.md` (176): they describe a direct float.

## 2. Does a page explain `requires`?

Yes. Three pages carry it today, split by audience:

| Page | Role |
|---|---|
| `guides/dependencies.md` | How-to: bundle vs requires, with `grim why` (lines 31-44, 152-195, 262-270) |
| `artifacts.md#requires` (line 660) | Reference for the declaration grammar |
| `concepts.md#dependencies` (line 241) | Explanation of what resolution does: conflicts, cycles, limits, offline |

`.agents/plans/plan_artifact_dependencies.md` is where the dependencies ADR's docs work was planned. I did not read it in full. The guide is the page it produced.

The one home for the claim grammar is `artifacts.md#requires`, as a new "What the tag claims" subsection (item 3 above). `concepts.md#dependencies` and `#bundle-conflicts` carry the resolution rules and link back to it. The how-to guide cannot be the home: it is a how-to and should not hold a grammar table, so it just links.

## 3. Docs gates

`task docs:check` is alias `docs:verify`, defined in `/home/mherwig/.herdr/worktrees/grimoire/uno/taskfiles/docs.taskfile.yml:69-94`. It runs, in order:
1. `docs:build`: `schema:generate`, then `npm ci` and `astro build`.
   - The build includes `starlight-links-validator` (`docs/astro.config.mjs:115`), so a dead internal link or anchor fails the build.
   - Schemas under `docs/public/schemas/` are gitignored and regenerated from Rust doc comments.
2. `docs/check_urls.py`: every shipped path, schema `$id`, `{#custom-id}` anchor, and the catalog/README links.
3. `.claude/rules/docs-quality/checks/doc_declaration.py` over `docs/src/content/docs`.
4. `docs/theme_check.sh`.
5. `docs/landing_check.py`.
6. `npm audit --audit-level=high`.
7. `npm test --if-present`.

Node must be 24 or newer (`docs/package.json:6-8`, `docs/README.md`).

A docs edit must pass the declaration rule. Keep each existing page's `<!-- doc_type: … -->` within the first 12 lines; do not move it above frontmatter. New pages need one, and `how-to`, `tutorial` and `landing` also need `doc_tier`.

`docs-quality.md` lists more gates that `docs:check` does not run:
- `prose.py` (sentences under 25 words)
- `page_type.py`
- `landing_check.py`
- `doc_examples.py --changed-only`
- `nav_depth.py`
- `links_raw.py`
- markdownlint

It says to enforce them at error on changed files. Style rules from `docs-style.md`:
- reference-style links only, with definitions at the bottom of the file
- `{#kebab}` anchors on cross-linked headings
- state exact behaviour, not simplifications

`clients.md` is code-mirrored (table parity in `src/install/client_target.rs`). Nothing here touches it.

## 4. Catalog skills

All paths below are under `/home/mherwig/.herdr/worktrees/grimoire/uno/catalog/`.

**`skills/grim-usage/references/consume.md`**
- **Lines 151-160**, the held-back paragraph: "A direct declaration's pin also wins over what a dependency asks for." Add that a bare line yields and that `--allow-downgrade` exists.
- **Lines 182-186**: "Every other dependency pin stays while the lock's hash matches". Still holds. Add the claim-satisfaction case.
- **Lines 504-509**: "A **dependency row** answers by the tag its pin records…". Same `status --check` change as `commands.md:1482`.
- **Lines 684-689**, bundle conflicts ("a direct declaration always wins over any bundle… disagreeing bundles fail closed"): bundle-member claims stay literal. It still holds for bundles. Check that the sentence says so.
- **Lines 743-746**, Dependencies: "A **direct declaration wins** over a dependency edge. Two edges that name different digests for one slot fail closed with exit 78". This is the main stale line. Add the tag-is-a-claim rule, the bare-line yield, the `--allow-downgrade` refusal, and the new 78/79/65 refusals.
- **Lines 273-277**, `update`: "a dependency keeps its pin even after its tag moved". Add the claim line.
- **Line 99** of `SKILL.md` (`grim update`) and **line 97** (`grim lock`): the command rows. Add `--allow-downgrade` and keep them under the Tier 2 rule (narrative only, "confirm with `grim <cmd> --help`").

**`skills/grim-usage/references/troubleshooting.md`**
- **Line 33**, the exit-78 row: "bundle conflict". Add "version-claim conflict, or a major downgrade".
- **Line 34**, the exit-79 row: add "no release satisfies a claim".
- **Lines 47-52**, the `reason` list: add `major-downgrade`.
- **Lines 212-218**, "A Held-Back Request": add the yield and the downgrade refusal.
- Also add the 65 budget refusals to the exit-65 row near line 28.

**`skills/grim-usage/references/publish.md`**
- **Lines 57-60 and 76-77**, Cascade Tags: "Floating tags (`1.2`, `1`, `latest`) move freely on every cascade — that is their job." The release guard contradicts "move freely". Rewrite per Q1 (a).
- **Lines 393-404**, bundle pin and dependent artifacts: add the claim grammar if you describe `requires` tags.

**`skills/grim-authoring/SKILL.md`**
- **Lines 62-81**, Declaring Dependencies: "a binding name to a reference, relative (`./shared:1`) or fully qualified". Add what the tag claims, one or two sentences, and link to `artifacts.html#requires`.
- **Lines 78-79**: "Never add them to a version tag consumers already float on". Unchanged.

**`skills/grim-authoring/references/release-checklist.md`**
- **Lines 113-117**, Cascade tags: "Releasing `1.2.3` also moves the floating tags `1`, `1.2`, and `latest` to the new digest". Add the no-lower-version guard.

**`skills/grim-authoring/references/bundle-spec.md`**: lines 108-120 (Floating vs. Pinned) and 165 (`BundleConflict` row for two bundles disagreeing on a member). Bundles keep literal tags, so check only. No change likely.
**`skills/grim-authoring/references/skill-spec.md`**: line 129, "Pin the fallback to a floating major tag (`:1`)". Still correct.
**`mcp-spec.md:179-184`** and **`hook-spec.md:500-509`**: the Requires sections. They link back to the SKILL section; no change.

**No change** (checked by grep): `ai-config-authoring` (vendor-neutral, no mention of grim dependencies), the `grim-essentials` bundle and the `grim` MCP server. The bundle's members `../skills/<name>:0` are literal bundle-member tags, unchanged by Decision 1. The MCP descriptor has no `requires`. The comment in `bundles/grim-essentials.toml` about "cascade tags + `grim update`" stays true. Mention only if you want the backport guard noted in `catalog/README.md:~76`, "Semver releases cascade (`1.2.3` also moves `1.2`, `1`, `latest`)".

**`task catalog:verify`** (`taskfile.yml:21-60`) runs `grim build` on every skill directory, rule, agent, MCP `.toml`, hook directory and bundle. The real parser is the schema gate, so it catches a malformed document but not stale prose. Drift is reviewed by hand. `catalog/README.md` "Keeping content honest" says to re-review `grim-usage` and `grim-authoring` whenever `docs/.../{artifacts,publishing,commands,configuration,json-interface,stability,…}.md`, `src/command/**` or `src/mcp/**` change. `ai-config-authoring` is reviewed only for `clients.md` and `vendor-metadata.md`. Each package's `references/updating.md` holds the re-research procedure. This change touches `artifacts`, `publishing`, `commands`, `configuration`, `json-interface`, `stability` and `src/command/**`, so the review is required.

## 5. Help text and schemas

Schemas are gitignored and generated from Rust, so the edit goes in the source:
- **`/home/mherwig/.herdr/worktrees/grimoire/uno/src/command/schema.rs:159`** builds the `requires` description for `grim-mcp.schema.json` and `grim-settings.schema.json`: "Artifacts this {owner} requires: one table per kind (…), each mapping a binding name to a member reference." It says nothing about what a tag means. Add one clause such as "a tag is a version claim: `2.1` means 2.1.0 or newer in 2.x; no tag or `latest` means any version".
- **`src/command/testdata/settings.schema.json:2167`** is a committed golden copy of that same description. Update it together with `schema.rs`, or the schema test fails.
- `grimoire-lock.schema.json`: `LockedArtifact.requires` ("Declared dependency edges, as published") and `pinned` ("Resolved registry/repo pinned to a content digest, with the tag that resolved it when there was one") stay accurate. No schema change is made.
- `grimoire-config.schema.json` has no `requires`, and no tag meaning to update.
- `grim-publish.schema.json`'s `pin` ("Freeze every floating reference to a digest…") is unchanged.

Clap doc comments (the source of `--help`):
- **`src/main.rs:101-117`**: `Lock` "Pin new declarations in `grimoire.lock` and keep existing pins; `grim update` moves them", `Update` "Re-resolve floating tags and re-materialize changed artifacts", `Release` "Validate, pack, and push a skill/rule with cascade tags", `Add` "Declare a skill/rule in the config and lock it". Only the `--allow-downgrade` flag docs change; these one-liners can stay.
- **`src/command/release.rs:82-92`**, `--cascade` and `--no-cascade`: "move `X.Y`, `X`, and `latest` onto this release". Add "never to a lower version".
- **`src/command/add.rs:97-101`**, `--force`: "Overwrite a locally modified artifact". The ADR's Q3 asks whether this scope covers a moved dependency (F1 says it does not). Settle that before touching the help text.
- **`src/cli/options.rs`**: add `DowngradeOpts`, flattened into `AddArgs`, `UpdateArgs` and `LockArgs`. It follows `HookTrustOpts` (line 146). The flag description is new text. `LockArgs` has no fields today (`src/command/lock.rs:42`).

## 6. AI rules (`.claude/rules/`)

- **`subsystem-cli-commands.md:21`**, `grim add`: the long row says "`--force` … scoped by `lock::closure::force_nodes` to the target or a bundle's members… never a dependency it merely requires". That matches the ADR. Add `[--allow-downgrade]` to the synopsis and the major-downgrade 78.
- **`subsystem-cli-commands.md:22`**, `grim lock`: "**without moving a pin**… so a node resolves only if it is new or its request no longer matches the pin". Add that a pin now stays while its evidence satisfies every Range claim, that `--allow-downgrade` exists, and that a pin below a raised lower bound moves with no flag. Also "A hand-deleted direct line … stays at exit 0 where a fresh resolve exits 78".
- **`subsystem-cli-commands.md:34`**, `grim update`: "Re-resolve floating tags, roll the lock forward". Add `--allow-downgrade`.
- **`subsystem-cli-commands.md:40`**, `grim release`: "validate, pack, push with cascade tags". Add the float guard.
- **`arch-principles.md:64`**, the `adr_artifact_dependencies` row: "direct wins, equal digests merge, different digests 78". Decisions 3 and 8 amend this. Say so, and add the new ADR row. `grep` found no `adr_dependency_version_claims` entry in `.claude/rules` or `.agents` other than the ADR itself and its research file.
- **`arch-principles.md:65`**, `adr_lock_pin_stability`: the pin rule and D1. The ADR amends the pin rule (Range rows) and D1 (which tag a moved pin records).
- `subsystem-install-state.md`, `subsystem-file-structure.md`, `quality-rust-exit_codes.md`, `subsystem-cli-api.md`: no stale statement found. `subsystem-cli-api.md:102` mentions `reason`, and the new `reason` literal should be listed there if the rule keeps a literal inventory (it does not appear to).

## 7. Changelog and release notes

- `CHANGELOG.md` is git-cliff output from commit subjects. It has no `Unreleased` section and the top entry is `[0.15.1] - 2026-10-28`. Do not edit it by hand.
- `docs/src/content/docs/changelog.md` is the user-facing release write-up (`doc_type: changelog`). Its newest section is `## 0.15 — … {#v0-15}`, and 0.16 has no section yet. It is written at release time by the `changelog` skill after `task release:prepare` (`.claude/skills/changelog/SKILL.md`). Headings inside a version section use `####`. Do not add 0.16 text now.
- `upgrading.md` is where an unreleased behaviour change lives. It already has several 0.16.0 sections (lines 459, 521, 625, 1154), and the version in `Cargo.toml` is still 0.15.1. The claim semantics belong in the existing `#dependencies-need-new-grim` section. The `grim release` float guard needs a new row because it changes a shipped command.
- Commit convention: `docs:`, `feat:` and `fix:` commits reach the generated changelog. `chore:` is for `.claude/`, skills and rules, which keeps them out of it (`workflow-git.md:52`).

## Table

### (a) Docs site (`docs/src/content/docs/`)
| File | Section | What changes |
|---|---|---|
| `artifacts.md` | `#requires` 660-736 | New "What the tag claims" subsection: grammar table, any-version rule, literal tags. Bundle members stay literal (574-577) |
| `concepts.md` | `#bundle-conflicts` 186-223 | Narrow "direct always wins" (192-196). Agreement is newest in major (197-199). Add two-majors 78 and `--allow-downgrade` (200-211) |
| `concepts.md` | `#dependencies` 277-289, limits 298-312, offline 314-345 | Claim semantics replace "Floating by default"; "Your direct declaration wins" narrowed; two new limits; listing is an offline miss |
| `commands.md` | `add` 458-463, 593-603; `lock` 638, 722-745; `update` 911, 1165-1174; `status --check` 1469, 1482-1498; `release` 4012-4034, 4085-4102; 1340-1342, 1686-1691 | `--allow-downgrade`; new 78/79/65 rows; `status --check` answers by `T(line)`; release float guard; yield caveat on `overrides:` |
| `json-interface.md` | 1039-1045, 1078-1088, 1136-1142, 489-494, 172-177 | Exit rows; `major-downgrade` reason (not forceable); `status --check` dependency row; `moved` disposition |
| `stability.md` | 32, 42, 131-147, 209-212, 225, 313-315 | Record the new flag, reason, release guard, and the diamond change; adjust the "Eleven" count if items are added |
| `upgrading.md` | 459-519 (extend) plus a release-guard row | Diamond within one major, bare-line yield, recorded cascade tag, `--allow-downgrade`, float no longer moves down |
| `publishing.md` | 694-707, 987-996, 961-970, 956-959 | Cascade guard; replace "conflict" line; ordering and `--pin` clauses |
| `configuration.md` | 235-241, 907-913 | Narrow direct-wins; a dependency may record a cascade tag |
| `guides/dependencies.md` | 262-270, 69-71 | Bare-line yield and a worked two-requesters example; one sentence on claim meaning |
| `guides/versioning.md` | 12-27, 69-72 | `requires` tag is a claim; cascade guard |

### (b) Catalog skills (`catalog/skills/`)
| File | Section | What changes |
|---|---|---|
| `grim-usage/references/consume.md` | 151-160, 273-277, 504-509, 743-746; `SKILL.md` 97, 99 | Claim grammar, yield, downgrade refusal, `status --check` answer, flag mention |
| `grim-usage/references/troubleshooting.md` | 28, 33, 34, 47-52, 212-218 | Exit rows; `major-downgrade` reason; held-back and downgrade guidance |
| `grim-usage/references/publish.md` | 57-60, 76-77 | Floats "move freely" no longer true; float guard |
| `grim-authoring/SKILL.md` | 62-81 | One or two sentences on what a tag claims, plus link |
| `grim-authoring/references/release-checklist.md` | 113-117 | Cascade float guard |
| `catalog/README.md` | ~76 cascade line | Optional clause on the guard |
| `ai-config-authoring`, `grim-essentials`, `grim` MCP | none | No change |

### (c) Help text and schemas
| File | Section | What changes |
|---|---|---|
| `src/command/schema.rs` | 159 | Add tag-is-a-claim clause to the `requires` description (MCP and settings schemas) |
| `src/command/testdata/settings.schema.json` | 2167 | Update the golden copy with it |
| `src/command/release.rs` | 82-92 | `--cascade`/`--no-cascade` doc: never moves a float to a lower version |
| `src/cli/options.rs`, `add.rs`, `update.rs`, `lock.rs` | new `DowngradeOpts` | New `--allow-downgrade` help text on three commands |
| `src/main.rs` | 101-117 | One-liners can stay; check against Q3 for `add` |

### (d) AI rules
| File | Section | What changes |
|---|---|---|
| `.claude/rules/subsystem-cli-commands.md` | rows at 21, 22, 34, 40 | `--allow-downgrade`; lock pin stays while evidence satisfies; release float guard |
| `.claude/rules/arch-principles.md` | ADR rows 64, 65 | Mark Decision 3 and pin-rule amendments; add a row for the new ADR |

**Not checked:** I did not open `.agents/plans/plan_artifact_dependencies.md`. `src/main.rs` and the `subsystem-cli-api.md` literal inventory got only a grep.

**Open questions that change wording:**
- Q1: the `grim release` float guard and the "newest in major" caveat. Docs and catalog say the guard exists only if (a) wins.
- Q2: the `commands.md:638` "the same tag" sentence. It changes only if a bare line may be answered by any tagged pin.
- Q3: the `add --force` scope text.
