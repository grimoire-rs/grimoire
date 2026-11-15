# Plan: lock-pin-stability follow-ups (#330, #331, #332, #337, #360)

## Status

- **Plan:** plan_lock_pin_followups
- **Active phase:** 2 — Graph split
- **Step:** /hex-execute → Implement
- **Last update:** 2026-11-15 (after 8d81e92e: chore: split long sentences in the lock pin ADR)

---

## Context

Five open follow-up issues from the lock-pin-stability and dependency-version-claims work:

- [#337](https://github.com/grimoire-rs/grimoire/issues/337) docs: gone-tag exit sentence and loose kept-pin wording
- [#330](https://github.com/grimoire-rs/grimoire/issues/330) docs: tighten wording on pick, add and relock pages
- [#332](https://github.com/grimoire-rs/grimoire/issues/332) refactor(lock): one keep rule and one requester-edge verdict
- [#360](https://github.com/grimoire-rs/grimoire/issues/360) refactor(resolve): split the Graph struct
- [#331](https://github.com/grimoire-rs/grimoire/issues/331) perf: measure and trim lock edit and part pick costs

Both parent features are on `main` (`002b15eb`), so the deferred cleanups are unblocked.
The issues were written against a pre-finalize commit. A scout pass against current code found
about half of the listed items already fixed on the way to `main`.
This plan does the remainder and closes all five issues from one PR.

No exit code and no byte of stdout or JSON changes. One stderr note in `grim add` is reworded, as #337 asks.

## Owner constraints

- Subagents run on `opus`.
- One feature branch, one PR.
- Linear commits, rebased on `origin/main`.

## Already done on `main` (no work; listed per issue in the PR body)

| Issue item | State on `main` |
|---|---|
| #337 `commands.md` gone-tag sentence | Sits beside its antecedent (~1373), before the held-requirement paragraph (~1378). Only the measured-cell check is open: D1. |
| #330 "dependent whose moved tag" (`stability.md`, `upgrading.md`) | Reworded; the moved tag is attached to the member. |
| #330 step-3 `grim add` 65 to 0 in the two lists | Present as bullet 2 of both lists. |
| #330 second route "declare a reference that exists" | Present in `commands.md` ~531 and `consume.md` ~78-82. |
| #330 "no longer lists exits 79" precision | Reworded with both cases (`consume.md` ~430). |
| #332 keep rule as Reach / Gone / Unknown | Done: `enum Reach`, `KeepRule` in `src/lock/edit.rs` (~406, ~437). |
| #332 `RequesterEdges` collapse | Type is gone. ADR amended (RF7, `adr_lock_pin_stability.md` ~352, live bullet ~164) to two rules; "one verdict" is no longer the recorded decision. |
| #332 `bundle_provenance_tag` beside `pin_rule` | Done: `src/lock/pin_rule.rs` ~427. |
| #332 `registration_remedy` to a shared module | Done: `src/install/hook_registrar.rs` ~1973. |
| #332 uninstall of a direct + bundle-provided skill | Handled by a stderr note naming the bundle (`kept_by_bundle_note`, `src/command/uninstall.rs` ~303) and an acceptance test. Ruling R1. |

## Work items

### Docs and wording (#337, #330)

- **D1** Measure the cell `commands.md` ~1373 claims: included bundle, plugin records no digest, tag gone → `79`. No test covers it (nearest: `test/tests/test_marketplace_pins.py` ~1210, ~1239, for a gone *requirement* tag). Add one test beside those with that module's fixtures. If the measured exit differs, fix the sentence, not the code.
- **D2** `src/command/add.rs` `refused_pin_note` (~846): "The manifest its tag names now does not resolve" is wrong where the cause is a version conflict (`kept_over_refusal`, `graph.rs` ~3889, is set on any first-walk error). Reword to cover refusal and conflict, e.g. "What its tag names now cannot be locked, and `{update}` says why". Same commit: the paraphrase in `commands.md` ~531 and every test or catalog page that quotes it (`grep -rn "does not resolve" src test docs catalog`).
- **D3** `catalog/skills/grim-usage/references/consume.md`: split sentences over 25 words, only on lines the lock-pin and version-claims commits wrote (`git blame`: `ee8ec9be`, `3b5ee406`, `7d2264ec`). The 100+ long sentences elsewhere in the file stay out of scope.
- **D4** `.agents/adr/adr_lock_pin_stability.md`: split the 28 to 30-word sentences in paragraphs ~117 and ~160.
- **D5** `src/lock/effective_set.rs` `reached_rows` doc (~126): add the retyped-artifact-line case (returns the stale row, whose edges the relock claims). ADR ~338 already says it.
- **D6** `commands.md`: run `python3 .claude/rules/docs-quality/checks/prose.py --root .` and split the over-long em-dash line beside the exit-81 row of the lock/install section (issue put it at ~1193 pre-drift; now roughly 1250-1500).

### Refactor #360: three structure-only commits in `src/resolve/graph.rs`

Field names stay identical inside the new structs, so the diff is `self.x` → `self.memo.x` / `self.attestation.x`. No new traits, no file split, tests stay inline. No other in-flight branch touches `graph.rs`.

- **G1 `RegistryMemo`** (`#[derive(Default)]`, after `struct Graph`): fields `digests`, `evidence`, `prefetched`, `listings`, `missing`, `listing_bytes`, doc comments verbatim.
  Methods that move, bodies byte for byte, receiver only: `tag_in_line`, `evidence_of`, `listed_tags`, `no_matching_version`, `annotation_against`, `prefetch_evidence`, `dropped_label`, `version_label`, `held_label`, `declare_held`.
  Stay on `Graph` (read other state): `resolve_digests`, `resolve_targets`, `line_candidate`, `judged_first`, `no_release`, `declare_tag`.
  Size: 22 method calls and 25 field accesses in the two `impl Graph` blocks; 20 path rewrites on 17 test lines.
  Traps: `evidence` is also a `CarriedPin` field (~940) and `missing` a `Fake` test-double field, so no blind sed. `kept_walk` (~3950) keeps its field-by-field copy; `kept.memo = self.memo.clone()` would also copy `prefetched` and change asserted read counts. Fix the intra-doc link at ~1203 by hand.
- **G2 `Attestation`** (`#[derive(Default)]`): `attested`, `unattested`, `confirmed`, `missed_offline`. Plain state struct; every method stays on `Graph`, because each reader also needs `nodes`, `carry` or the memo. 16 production path rewrites, 3 in tests.
- **G3 `edge_origin`** moves verbatim to `src/oci/version_claim.rs` beside `EdgeOrigin`. Layering holds: `NodeKey` and `ArtifactKind` already live in `crate::oci`. Import-line edits only in `src/command/why.rs`, `src/command/status.rs`, `src/resolve/resolver.rs`, `graph.rs`. No re-export.

"Tests pass unchanged" holds for names, values and assertions. 23 test lines get a path-only rewrite, listed in the commit bodies.

Grep gates per commit:

```
grep -nE "self\.(digests|missing|prefetched|listings|listing_bytes|evidence)\b" src/resolve/graph.rs   # only inside impl RegistryMemo, plus CarriedPin ~940
grep -nE "\.(attested|unattested|confirmed|missed_offline)\b" src/resolve/graph.rs | grep -v 'attestation\.'   # empty
grep -rn "fn edge_origin" src/   # version_claim.rs only
git diff --color-moved=dimmed-zebra --color-moved-ws=allow-indentation-change HEAD~1   # moved blocks show as moved
```

### #332 remainder: attestation guard

`settle_pass` (`graph.rs` ~4047) encodes "no release on an attestation change" as the short-circuit order
`release_held_out_of_line() || attest_direct_claims() || release_conflicts()`.
`a_pass_that_settles_a_line_releases_nothing` (~7341) reads as pinning it.
Confirm by mutation in scratch (not committed): swap `release_conflicts` ahead of `attest_direct_claims`, then `||` → `|`; the test must go red both times.
Red twice → item done, no commit. Green → add the smallest test on the same fixture (`settle_pass()` true, key still unattested, `released` and `conflict_released` empty).

### Perf #331: measure first, fix only what shows

Measurement, nothing committed, no new dependency:
- In-process: throwaway `#[ignore]`d `perf_scratch` tests in the existing test modules (helpers `moved_between`/`lock_holding`, `chain_of_requesters`, `lock_of`/`row`, `Fake` with its `resolves`/`manifest_fetches` counters). Median of 10 runs. Deleted before any commit.
- Whole command: `hyperfine` on the release binary against a private zot (`test/src/zot.py start_zot()`), zot log at `info` to count manifest and blob requests. Script lives in the scratchpad.
- Sizes: realistic 50 rows / 5 bundles; stress 500 rows / 50 bundles; item 4 adds a 250-member bundle dropped from 500 rows; TUI batch M = 5 and 20.

Threshold (ruling R2):
- Registry reads: any avoidable request whose count grows with N or M shows.
- Local CPU and store reads: shows at ≥ 50 ms at stress, or ≥ 10 % of its command at realistic size.

| # | Item | Verdict | Action |
|---|---|---|---|
| 1 | Part pick over a 0.15.1 part reads every bundle | Accept | Deliberate conflict guard (`resolver.rs` ~187; tests `a_pick_over_an_old_part_reads_every_bundle_that_lists_a_row_it_takes` and siblings). One-time: the pick stamps the digests. Reason recorded in the PR body. |
| 2 | `warn_relock` always calls `moved_pins` | Fix | Guard in `warn_relock` (`resolver.rs` ~768): skip `moved_pins` when `target.tagless_resolved` is empty. `moved_pins` is pure and its only consumer here, `tagless_warning`, returns before reading it. ≤ 5 lines, no test edit. Before/after numbers in the commit. |
| 3 | `StoreView::reach` shared visited set | Accept | The issue's fix is unsafe: the visited set is also the per-walk cap counter, and each dependent returns its own keys and `whole`. Sharing shrinks later dependents' keys and changes the `Unwalked` stderr note. Blob reads are already memoized per view and pinned at ≤ 5N by `an_edit_reads_a_chain_of_requesters_a_linear_number_of_times`. Record the measured time. |
| 4 | `KeepRule` nested scans | Measure, then decide | If it shows: precompute edge-target → parent rows once in `KeepRule::new`, as a `Vec` in lock order (first match feeds note text; never a sorted set). Own `perf(lock):` commit with numbers. |
| 5 | TUI batch delete M × N | Measure, then defer | One relock per batch changes per-row status, ledger and flock semantics. Too large for a follow-up PR. Numbers go into a new issue (ruling R3). |
| 6 | `Graph::confirmable` and the kept walk | Measure, expect accept | No registry I/O; second walk repeats no tag read. Seeding it from the first walk risks dropped warnings. Record the numbers. |

## Rulings I made (mine, not the owner's)

- **R1 — uninstall status for a direct + bundle-provided skill.**
  Situation: `grim uninstall skill x` where `x` is declared directly and a declared bundle also provides it. The declaration is dropped, the files stay, JSON says `"status": "uninstalled"`, and a stderr note names the bundle to remove.
  Ruling: leave it; the item is closed as done.
  Alternative: an additive JSON field such as `"provided_by": ["team-bundle"]`, so a script sees why files stayed without parsing stderr. Skipped: it widens a frozen surface and nobody asked for the machine-readable form.
- **R2 — what "shows" means for perf.**
  Situation: the issue says "fix only what shows" and gives no bar.
  Ruling: the threshold above, with item 2 fixed regardless because the issue names that exact guard and it has no semantic risk.
  Alternative: fix every item listed. Example cost: item 3's proposed fix would change a stderr note.
- **R3 — TUI batch cost goes to a new issue.**
  Situation: a batch delete of M rows relocks M times.
  Ruling: measure, then file a new issue with the numbers; #331 closes.
  Alternative: do one relock per batch here. Example cost: today a version conflict on row 3 stops only row 3; a single relock would fail the whole batch.
- **R4 — `RegistryMemo` owns its fill logic.**
  Situation: #360 says "the lookups that only read it".
  Ruling: move every method that touches only memo fields, including the four async ones that fill it.
  Alternative: move `&self` methods only and leave the four fillers on `Graph`; smaller raw diff, but the memo's writes stay scattered.

## Branch, commits, PR

Branch `refactor/lock-pin-followups`, created in this worktree from `origin/main` (tree is clean; `feat/rule-support-dir-removal` stays as a committed branch).
All commits signed off (`git commit -s`), Conventional Commits, no merge commits.

1. `chore: plan the lock-pin follow-ups` — this plan as `.agents/plans/plan_lock_pin_followups.md`
2. `test(pick): measure a gone included-bundle tag over a digestless plugin` (D1; plus the sentence fix if the cell differs)
3. `fix(add): word the refused kept-pin note for a version conflict` (D2)
4. `docs: split long sentences on the lock and pick pages` (D3, D6)
5. `docs(lock): state the retyped-line case of reached_rows` (D5)
6. `chore: split long sentences in the lock pin ADR` (D4)
7. `refactor(resolve): extract the registry memo from Graph` (G1)
8. `refactor(resolve): extract the held-pin attestation state from Graph` (G2)
9. `refactor(resolve): move edge_origin beside EdgeOrigin` (G3)
10. `perf(resolve): read moved pins only for a tagless warning` (item 2, numbers in the body)
11. optional `perf(lock): …` (item 4, only if it shows)

Final step, covered by approving this plan: `git fetch origin && git rebase origin/main`, push the branch, open one PR.
PR body: per-issue table (done on main / done here / accepted with reason and number), `Closes #330, #331, #332, #337, #360`.
If item 5 shows, file one new issue for the TUI batch cost and link it from the PR. Merge stays the owner's.

Overlap to expect: `hex/config-artifact-type` also edits `resolver.rs`, `add.rs`, `effective_set.rs` and the three docs pages. Our edits there are a guard, a string and a doc comment; whichever lands second rebases.

## Execution

Opus workers, serial, in this one worktree with one build dir. Each worker commits its own slice; I read each diff before the next starts.

1. Worker A: D1 to D6 (commits 2 to 6).
2. Worker B: G1 to G3 (commits 7 to 9) plus the #332 mutation check.
3. Worker C: perf measurement, commit 10, commit 11 if it shows; returns the numbers table.
4. One opus review pass over the whole branch diff, focused on behaviour preservation. One fix round, no loop.

## Verification

- Per Rust commit: `task rust:format:check`, `task rust:clippy:check`, `task rust:test:unit -- resolve::` (commit 9 adds `oci::version_claim command::`).
- D1 and D2: the touched pytest modules via `task test:…`; `task catalog:verify`; `task docs:check` (Node 24); `prose.py` on the touched pages.
- Commit 10: stdout and stderr of `grim lock`, `update`, `remove`, `uninstall` diffed before/after, plain and `--format json`; byte-identical.
- Commit 11, if it lands: `GRIM_OLD_BIN=… task test:old-grim`.
- Final gate: `env -u OCX_GLOBAL task verify` on the rebased tip.
- Windows: no local Windows on this host; CI on the PR is the proof. Compare any red against `main`'s known Windows failures before blaming the branch.
