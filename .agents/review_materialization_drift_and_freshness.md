# Code Review: `feat/materialization-drift-and-freshness`

Tier high, `/hex-review`, 2026-08-12. Baseline `main`. Diff 26 files, +1794 / -285, 8 commits.
Panel: 8 workers (2 Stage 1, 6 Stage 2). Cross-model gate **skipped** — see below.

Worker outputs (scratchpad, not committed):
`stage1-spec.md`, `stage1-testcov.md`, `stage2-{architect,security,performance,docs,ux,research}.md`

---

## Summary

- **Verdict: Request Changes**
- Cross-model: **skipped** — `codex:rescue` forked, spawned a nested Codex task, and never wrote its output file (300s bounded wait). Fourth recorded failure of this adversary. One review layer is missing from this run.
- Counts after max-wins dedup: **7 Block · 10 High · ~15 Warn · 5 Suggest**
- Four independent Request-Changes triggers fired: unresolved Blocks; a systemic cause affecting ≥3 findings (one cluster holds 7); new behaviour without tests; an unjustified constitution violation.

### The one-paragraph version

The engineering is good and the central instinct is right — `grim update` silently
destroying hand-edited work was a real defect. Two things block it. **The contract
paperwork is inverted**: the branch labels itself `**BREAKING**` twice, when the
shipped docs on `main` already promised the *new* behaviour, so this restores the
contract rather than breaking it — but the label as written self-declares a
Principle 9 violation with no deviation record, which is an automatic Request
Changes. **And the docs describe a feature the code does not implement**:
`outputs_pending` cannot detect a deleted file, a claim that ships in six places
including the frozen 1.0 stability contract.

---

## Blocks

**B1 — `src/install/expected_outputs.rs:81-93` — `outputs_pending` cannot detect a deleted output, which is one of its three documented causes.**
`is_covered` never touches the filesystem: it compares recorded client + anchored path only. No `exists()` or `canonicalize` appears anywhere in the module (verified). So `rm .claude/rules/rust-style.md` then `grim status --format json` yields `outputs_pending: []` while `grim install` would rewrite that file — a false negative in exactly the direction the module's own doc calls "worse than no signal at all" (`:9-11`).
The false claim ships in **six** places: `expected_outputs.rs:57`, `src/api/status_report.rs:107`, `docs/src/json-interface.md:89`, `docs/src/stability.md:155` (**the frozen 1.0 contract**), `catalog/skills/grim-usage/references/consume.md:241`, `docs/src/commands.md:624`, `.claude/rules/subsystem-cli-commands.md:25`.
Decisive evidence: grim already has the capability. `ClientOutput::is_present(roots, containment)` exists at `src/install/install_state.rs:150`, and `footprint()` at `src/command/status.rs:768` **already calls it at `:776` on the same `ClientOutput` records, in the same command**. The module doc's "filesystem detection is unsound as an oracle" argument is sound for *client presence* and does not extend to "does this recorded path still exist".
*Remediation:* either add the existence probe (reuse `is_present`) plus a test, or delete the claim from all six surfaces and say a deleted output surfaces as `state: missing`. **Deferred:** the owner picks which, because one of the six is the frozen stability contract.
*Found independently by three workers (spec, testcov, researcher) plus a doc-reviewer extension.*

**B2 — `docs/src/json-interface.md:89` + `src/api/status_report.rs:103` — documented "sorted by client"; the code never sorts.**
`pending_outputs` returns in `expected_clients` order; `target.clients()` (`src/install/target.rs:169`) returns the stored slice with no ordering guarantee. The sibling field `client_drift` both sorts explicitly and has a determinism test (`status.rs:1191-1193`); `outputs_pending` has neither. An unguaranteed, untested ordering promise on a frozen JSON surface.
*Remediation:* sort, and add the determinism test the sibling already has.

**B3 — `CHANGELOG.md:12` and the `BREAKING CHANGE:` footer on commit `094db20` — a compliance-restoring defect fix is self-labelled a Principle 9 violation, with no deviation record and the additive route unevaluated.**
`main`'s own shipped docs already promised the new behaviour: `commands.md:532` ("Shares `--client` and `--force` with install"), `:472` ("`--force` overwrites a locally modified artifact instead of refusing it"), and `:545-547` ("This mirrors the install integrity gate, where a locally modified artifact is refused rather than overwritten without `--force`"). `main:docs/src/stability.md` contains no overwrite-on-update guarantee. `main:docs/src/json-interface.md:451` already documents a 65 path for `update`. **The old code contradicted the shipped doc**, so this is a fix, not a break.
Labelling it BREAKING converts a restoration into a self-declared Principle 9 violation — and there is no plan artifact on this branch, therefore no `Constitution deviations` table, which under the constitution gate is an automatic Request Changes. `**BREAKING**` last appeared at 0.9.0; four minor releases have honoured the freeze.
An additive route also exists and is never named, let alone rejected: skip the modified artifact, leave the bytes, report a new `action` literal, warn on stderr, exit 0 — which is *exactly* how update already handles its two other destruction paths (`kept-modified` for prune, `kept_modified_clients` for reap). Principle 9 requires the additive route be shown inadequate.
*Process note:* `taskfiles/release.taskfile.yml:60` regenerates `CHANGELOG.md` from commit history via git-cliff, and `main:CHANGELOG.md` has no `[Unreleased]` section. **The hand-written block is discarded at release — only the commit footer ships.** Any relabel must land on commit `094db20`'s footer, not just the changelog text.

**B4 — `CHANGELOG.md:21` — the TUI `u` change is labelled `**BREAKING**` on a surface the project explicitly excludes from the contract.**
`main:docs/src/stability.md:128-131`: "**TUI appearance and keybindings** … carry no compatibility promise — only exit codes and structured JSON output are contracts." The commits are already correct (`f1a8b27` carries no BREAKING footer); only the changelog asserts it.
*Remediation:* delete `**BREAKING**` from `CHANGELOG.md:21`. One line, unconditional.

**B5 — `src/tui/app.rs:2677` — the TUI half of the data-loss fix has no automated test.**
`perform` lost its `is_update` parameter and passes the user's answer straight through; same at `:2800` and `:2897`. All 93 tests in `app.rs` pass `force = false` over unmodified fixtures, so **restoring `is_update || force` at any of the three call sites keeps `task verify` green**. The CLI twin is guarded by two acceptance tests (`test_integrity.py:102`, `:132`); the TUI twin by none. The only record is a manual step in `test/manual/README.md:340-360`. Principle 2 is not met for the highest-severity of the five fixes.
*Remediation:* one `#[tokio::test]` reusing the real-workspace fixture the branch already built at `app.rs:6595` — hand-edit a materialized file, call `perform(..., false)`, assert `forceable_refusal` is `Some` **and the file still holds the edit**.

**B6 — `src/command/update.rs:306-313` — a refusal aborts `update` after every irreversible reconciliation has committed, and discards the report that is its only record.**
Verified ordering: `:129 lock_io::save` → `:157 install_all_with_progress` → `:196 persist_state` → `:214 prune_orphans` → `:243 reap_dropped_clients` → `:254 persist_state` → `:288 sync_config` → `:306 build_report` → `:312 return Err`.
This path is reachable **only because this diff changed the hardcoded `true` to `args.force`** — on `main`, `Refused` was unreachable from `update`. `install.rs:401-404` has the same build-then-discard shape and is safe there because `install` mutates no lock, prunes nothing and reaps nothing.
*Impact:* `grim update` over 40 artifacts, one with a stray trailing-newline edit — lock rolled forward, 39 re-materialized, a member pruned, a client reaped, then exit 65 with **no report**. `reaped_clients`, `kept_modified_clients`, `retained`, `abandoned_entries` exist only in the discarded report. CI reads 65 and rolls back a workspace that was fully updated; re-running returns 65 again, so the operator cannot distinguish "nothing happened" from "everything happened".
*Remediation:* detect the refusal **before** the first irreversible write, or carry it through the report with a non-zero `ExitCode` so the structured output survives.
*Found by architect (Block), corroborated by UX (High) and security (Warn).*

**B7 — `catalog/skills/grim-usage/references/troubleshooting.md:140-157` — the catalog skill's "Integrity Gates" index omits this branch's headline change.**
The section whose job is to enumerate grim's overwrite-refusal behaviours does not mention that `grim update` now refuses a locally-modified artifact. This is the drift review `AGENTS.md` mandates for `src/command/**` changes. **`task --force catalog:verify` passes clean** — the gate checks structure, not semantics, so it cannot catch this.

---

## High

| # | Location | Finding |
|---|---|---|
| H1 | `src/context.rs:291-329` | The memoized OCI client keeps the **first** credential seen per registry for the life of the `Context` — for `grim mcp`, the whole process. `grim logout` or a PAT rotation is never honoured until restart; Basic tokens never expire (`token_cache.rs:118-119` → `u64::MAX`). Grim re-reads the credential store on every operation (`registry_client.rs:349,371,395,448,475,521`) and the forked client then discards it (`external/rust-oci-client/src/client.rs:440-444`). Diff-introduced: before, a fresh client per call meant an empty `auth_store`. *Fix:* bound the memo's age (~5 lines, `(Instant, Arc<dyn OciAccess>)`), and state the bound in the `clients` doc comment at `:67-85`. |
| H2 | `docs/src/stability.md:157-158`, `docs/src/commands.md:635` | "cannot disagree with what `grim install` then does" and "Remediation is `grim install`, which clears it" are false on two reachable paths. |
| H3 | `docs/src/upgrading.md:7-10` | The page defined to carry exactly this class of change was not updated, and now contradicts the stability page this branch edits. |
| H4 | `docs/src/commands.md:1106-1267` | The `grim tui` reference section was never updated for the new Overwrite dialog on `u` or the `pending`/`+` badge — despite this same file receiving 41 lines of edits elsewhere in this diff. |
| H5 | `src/tui/render.rs:950` | The status-bar legend is width-gated against a stale hard-coded string, so it renders clipped and drops the `pending` explanation it was added to carry. |
| H6 | `src/install/install_error.rs:74-77` via `update.rs:311` | The refusal tells the user to "rerun with `--force`" — but on `update` that flag also authorizes prune and reap deletions the message never mentions. |
| H7 | `src/api/artifact_status.rs:17-29` vs `src/install/status_badge.rs:26-41` | The code comment asserting the two enums agree is now false. (Note: the docs are *correct* here — `state` legitimately excludes `pending`, which is a `search` badge value only.) |
| H8 | `src/command/search.rs:~305` | The `pending` badge is never exercised through `grim search`; every `BadgeContext` in the suite uses `target: None`, the arm that yields the old four-badge behaviour. |
| H9 | `docs/src/json-interface.md:452-453` | `modified`/`untracked-destination` reason rows name only `install`/`add` as the retry; `update` now emits both. (Escalated Warn→High by the UX perspective.) |
| H10 | `src/install/installer.rs:791-793` | **Deferred, pre-existing.** `--force` does a hard `remove_path` then rewrite with no backup — the user's edit is unrecoverable. dpkg (conffile prompt) and rpm (`.rpmsave`/`.rpmnew`) both preserve both copies. *Not introduced by this diff* — but this diff makes `--force` the prescribed remedy for a new refusal path, routing more users into it. |

---

## Warn (abridged — full detail in the worker files)

Test gaps: `would_overwrite` client-half only (`installer.rs:1253`); the `Err(_) => true` arm unreachable from any test (`:1257`); `tui/event.rs:725` sync test does not read the gate it guards; `render.rs:1946` — 5 of 10 needles cannot fail; `tree.rs:126` `Rollup::worst()` rung uncovered; bundle rollup covers 2 of 5 documented ranks; 3 of 4 `outputs_pending` producers untested; `Pending → ViaBundle` unreachable.

Correctness / design: `expected_outputs.rs:100-105,119` — the fail-open's doc justification is factually wrong and the arm produces a spurious refusal on the installer path; `status.rs:594-601` — `outputs_pending` paths are raw joins while siblings are anchored; `status.rs:216,320` — two undocumented policy decisions (declared-bundle row hardcoded `[]`, dev-install row computes it); `installer.rs:1251-1257` — the narrowing changes `install`'s shipped refusal behaviour (65 → 0 for one case) with no test and no doc anchor.

Perf: `context.rs:291-328` — a `### Performance` changelog claim with no measurement recorded (Verification Honesty); `context.rs:22,302-328` — the memo is unbounded and never evicted; `app.rs:1474-1491` — the bundle rollup turns a per-parent read into per-member work.

Docs / hygiene: `render.rs:1461` — the `?` overlay's Pending legend names 1 of 3 causes and says "configured" despite the documented autodetect carve-out; `render.rs:1435-1445` — `esc` lost its overlay documentation; `render.rs:876-900` — `/ search` dropped from every hint tier of a catalog browser while `o open` keeps the widest; `commands.md:1180` — the TUI group-action table is now wrong about which states `i` acts on; `app.rs:1465-1500` — the rollup collapses "one member short one client" and "nothing materialized" into the same `+ pending`; `CHANGELOG.md:8-84` — `### Performance` is not a group git-cliff emits; `subsystem-cli-commands.md:24` — an unrelated `grim fetch` drive-by bundled into `955611d`.

---

## Root-Cause Analysis (rca=on, above Suggest)

**Cluster A — the docs were written from the design intent, never round-tripped against the code.** *(7 findings: B1, B2, B7, H2, H3, H4, H9)*
Why do seven documented claims contradict the code? They were authored alongside the design. Why never corrected? Nothing verifies a prose claim against behaviour — `catalog:verify` passes clean because it checks structure only. Why no such check? Because the JSON interface has acceptance tests for *shape* but none that assert the *guarantees* the docs state.
**Systemic fix:** for every documented cause or guarantee on a frozen surface, require one acceptance test that fails when the claim goes false. Start with the three `outputs_pending` causes and the "sorted by client" promise.

**Cluster B — the refusal path was bolted to the existing error site without walking the command's sequence.** *(B6, H6, and the `--force` widening Warn)*
The check landed at the end because on `main` that site only saw genuine `Err`; `install.rs` used the identical shape and was copied as convention. It is unsafe here because `install` mutates no lock, prunes nothing, reaps nothing.
**Systemic fix:** when aligning two commands' behaviour, enumerate what one does that the other does not *before* copying its control flow.

**Cluster C — a per-call construction became a per-process cache without re-examining lifetime.** *(H1, two perf Warns)*
The change was motivated and measured by the CLI case; `grim mcp` was named as the beneficiary but never analysed as a distinct lifetime regime.
**Systemic fix:** any cache added for the CLI path states its bound in terms of the longest-lived `Context` holder — which is `grim mcp`.

**Cluster D — the BREAKING label was chosen before the shipped contract was read.** *(B3, B4)*
Behaviour change was equated with contract break. The promise lives in `commands.md` prose, so the label needed the doc read and did not get it.
**Systemic fix:** before labelling BREAKING, cite the shipped doc line the change violates. If no such line exists, it is a fix. And apply the label to the commit footer, since that is what git-cliff ships.

**Cluster E — the TUI half of a cross-surface fix goes untested by default.** *(B5, H5, plus 4 TUI test Warns)*
The CLI twin had acceptance tests and that felt sufficient; the TUI call sites are separate code with their own `force` argument.
**Systemic fix:** a fix landing on both CLI and TUI needs a regression test on both.

---

## Owner decisions — 2026-08-12 (all deferred items settled)

1. **B1 — delete the claim.** No existence probe; the check is not worth the cost on the `search` and per-TUI-row paths. Remove the deleted-file cause from all six surfaces and document that a deleted output surfaces as `state: missing`. *(Noted and accepted: on the `status` path the stat is already paid — `footprint()` at `status.rs:768` calls `is_present` at `:776` on the same records. The cost objection holds for search/TUI, not status. Decision stands regardless.)*
2. **B3 — route (B), contract restoration.** Keep exit 65. It is not a breaking change: relying on the old behaviour was relying on a bug. Delete `**BREAKING**` from `CHANGELOG.md:12`, drop the `BREAKING CHANGE:` footer from commit `094db20`, and reframe `docs/src/stability.md:188` from "One deliberate break, in 0.13.0" to a contract-restoration note citing `main:docs/src/commands.md:472,532,545-547` as the shipped promise the old code violated. No deviation record needed — there is no deviation.
3. **B4 — delete `**BREAKING**` from `CHANGELOG.md:21`.** Same reasoning.
4. **H10 — fix it, as an atomic swap.** Not the rpm preserve-both design. Write the new content to a temp path first; only once that succeeds, move the existing file aside and move the temp into place, then remove the aside copy. The user's edited bytes are still removed at the end — that is what `--force` means — the fix is eliminating the window where the file is missing or half-written. `.old` is **not** retained as a backup.
5. **`pending` naming — keep.** Rename not worth it; the value is already wired into the `grimoire-vscode` consumer.
6. **No transition path — accepted.** `--force` is the escape hatch and the refusal message prompts for it. No warn-first release.

**Execution scope authorized:** all Block (B1–B7) and all High (H1–H10) findings. Warn and Suggest not in scope for this pass.

**B6's shape follows from decision 2:** keep exit 65, but return the report alongside it rather than discarding it.

---

## Cross-Model Adversarial

**Skipped.** `codex:rescue` was invoked once (one-shot, per contract) with the brief written to a scratchpad file and passed as a pointer — the documented workaround for its two earlier failure modes. It forked, spawned a nested Codex task (`task-msqgfuaq-mu1qrd`), returned a "completed" notification, and never wrote its output file. Bounded 300s wait confirmed absence.

This is the **fourth** recorded distinct failure of this adversary in this repo. Per swarm memory's own instruction, it is reported as skipped rather than folded away: **this run carries one fewer review layer than tier high specifies.** The four findings it was briefed to falsify — B1, B6, H1, and the Principle 9 inversion — stand unchallenged by a second model.
