# Round 1 — test adequacy (rv-tests)

Verdict: **6 Block, 6 Warn, 8 Suggest.** Read all 9 new acceptance files in full plus conftest,
helpers, runner and the `test_status.py`/`test_config_registry.py` deltas; Rust side assertion-level for
`hook/{trust,policy,audit}.rs`, `hook_consent.rs`, the `prune.rs` diff, `pipeline.rs`'s roster and the
projector sanitize test. **Did not run the acceptance suite.** Name/presence-level only for
`json_splice.rs`, `vendor_*.rs`, `installer.rs`, `status.rs`, `client_target.rs`, `hook_launcher.rs`,
`hook_dispatch.rs`, `envelope.rs`, `projector.rs`.

**Headline: the acceptance layer is strong on the real question.** Positive controls are real and
executed; all three named traps are genuinely closed (correlation id is now an equality at
`test_hook_run_runtime.py:271-277`; the workspace test enumerates permitted files instead of asserting
the vulnerable path; the B4/B5 fixtures move the entry to *global* config for their control). The gaps
are in the Rust unit layer and one uncovered end-to-end form.

## Block

- **B1** `src/oci/hook.rs:46,809,1443` — **grim recommends a handler form that never fires.**
  `argv = ["sh","${GRIM_HOOK_DIR}/guard.sh"]` is blessed by `payload_relative_file`, called *the* idiom
  in the module doc, and named in `PayloadNotExecutable`'s message — but nothing substitutes it:
  `desired_entries` copies `handler` verbatim and `handler_command` uses exec form. Executed:
  `argv[1]='guard.sh'` fires; `'${GRIM_HOOK_DIR}/guard.sh'` and `'$GRIM_HOOK_DIR/guard.sh'` do not,
  silently, exit 0. `command = "sh ${GRIM_HOOK_DIR}/guard.sh"` works (goes through `sh -c`); only the
  **documented preferred** form is dead. `test/src/helpers.py::make_hook` emits exactly the dead form,
  so the first test that arms a `make_hook` artifact gets a silently-inert hook.
- **B2** `src/hook/trust.rs:406` `grants` — the **path-segment-boundary rule has no test.** The doc
  table states `ghcr.io/acme` must not grant for `ghcr.io/acme-evil/guard`; nothing asserts it. Drop the
  `starts_with('/')` guard and every test stays green while consent leaks across a namespace boundary.
- **B3** `src/hook/trust.rs:327,459` — the W8 `insecure` clause and `is_loopback` have **zero coverage**.
  No test sets `insecure = true` on a hook-bearing entry, and the acceptance registry is `localhost:5000`
  so `is_loopback` short-circuits anyway. Deleting condition 5 breaks nothing.
- **B4** `src/hook/audit.rs` — **rotation and the record cap have no tests.** 738 lines, 3 tests, all on
  `append_all`/`writable`. Untested: `rotate_if_needed`, `rotated_path`, `ROTATED_SUFFIX`,
  `MAX_LOG_BYTES`, and the whole `capped_line` elision ladder. The stated `2 * MAX_LOG_BYTES` bound rests
  on untested code, and a record that silently fails to fit is an unlogged invocation.
- **B5** `src/command/hook_consent.rs` — **259 lines, 0 tests.** Untested at every level: the
  bundle-delivered-hook disclosure (and whether its position after the `feature_enabled()` early return
  is intended), and `persist`/`persist_grant`/`namespaced_locator` — the B5.2 narrowing whose regression
  to one host-wide entry would silently grant consent to every publisher on the host.
- **B6** `test/data/golden/pre_hooks_03e59b0/` — **contract C-015 has no consumer.** ~1,500 lines of
  baseline data plus `tools/verify.py`, referenced from no test, taskfile or workflow.
  `.agents/golden_fixture_generation.md:260` says so outright.

## Warn

- **W1** `test_hook_decline_dispatch.py:211` — the P-1 spawn negative has **no in-test positive
  control**; the registered sibling writes nothing observable, so a dispatcher that degraded on the table
  read passes.
- **W2** `test_bundle_hook_members.py:302,306` — the file's only arming assertions are a truthiness and
  a `!=`. **No acceptance test anywhere arms a bundle-delivered hook.**
- **W3** `test_bundle_hook_members.py:315` — the docstring names re-arming as the regression; the body
  never arms, so it pins only lock eviction. (Unit side in `prune.rs` is exemplary.)
- **W4** `test_hooks_boundary.py:475` — `_rows == []` after `grim add --allow-hooks` has no control; if
  `add`'s hook branch regressed to a no-op this stays green.
- **W5** `src/api/install_report.rs` — the new `armed`/`ArmedEntry` field is **never named by a test**;
  its only coverage substring-matches a `json.dumps` of the whole item. `armed` → `armed_for` is a
  breaking change on a released additive surface that no test catches.
- **W6** `test_hook_run_runtime.py:639` — truthiness where the value is the contract; passes if
  `permissionDecision` regressed from `deny` to `allow`.

## Suggest
1. `test_hooks_boundary.py:416` docstring over-promises (the control adds a namespaced entry, not the
   bare host it claims); the promised case exists incidentally at `test_hook_decline_dispatch.py:263`.
2. `test_hook_arming.py:748,367` — `!=` where `==` is available.
3. `test_hooks_lifecycle.py:366` — name over-promises; `state`/`arming` unchecked (pinned properly in
   `test_hook_list.py:107-134`).
4. `test_hook_arming.py:655` — the `as-grim-wrote-it` leg is confounded by `GRIM_OFFLINE=1`.
5. Two stale docstrings now misinform (`test_hook_run_runtime.py:898-905`,
   `test_harness_fixtures.py:154-161`).
6. `test_hooks_boundary.py:350` — `token.upper()` is a latent flake (~4e-7).
7. `test_hook_run_runtime.py:644-687` — `can_deny` reused for the negative leg.
8. The PowerShell registered form ships with **no executed test** (every hooks file is
   `skipif(os.name == "nt")`, CI is ubuntu-only). Needs a Windows runner — known gap. Deferred.

## Checked and adequately covered — do not ask again
Decision O ordering (11 tests, all four parts); multi-client union (write and read side, the read side
line-counting so a double spawn is caught); reap/uninstall incl. P-2's reap-can't-take-the-launcher with
a real armed sibling as control; the SEC-1 old-path reaper; both frozen-shape tripwires correctly
extended (13→14 status fields, 6→7 registry fields) and refusing to relax to a subset check;
`sanitize` via the projector test; the inverted defect-demonstrations are **real inversions** asserting
positive values, not relaxations; zero xfail confirmed across `test/tests/**`.
