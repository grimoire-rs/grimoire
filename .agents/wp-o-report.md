# WP-O — the hooks acceptance suite (S-001 … S-016)

Branch `hex/hooks-artifact-kind--wp6-o`, base `7bbc348`, one commit
(`test(hooks): close the S-001…S-016 acceptance matrix`).

Gate: `task verify` — **1090 passed, 3 xfailed**, clean. `.claude/tests/uv.lock`
and `test/uv.lock` reverted; `.claude/hooks/.state/commit-verified` written by
`task verify` itself.

## Coverage matrix

Every scenario is in exactly one of three states: **covered** (a test that fails
if the behaviour regresses), **defect** (an `xfail(strict=True)` asserting the
contract the plan states, so the fix is a loud failure whose remedy is deleting
the marker), or **not testable** with the reason stated.

| S-id | State | Test(s) | File |
|---|---|---|---|
| **S-001** gated ⇒ skip + warn + `gated`, exit 0 | covered (pre-existing) | `test_s001_gated_add_skips_with_a_warning_and_arms_nothing`, `test_s001_the_payload_is_not_materialized_while_gated` | `test_hook_arming.py` |
| **S-002a** entry ⇒ no prompt, it arms | covered (pre-existing) | `test_a_trust_hooks_entry_in_global_config_arms_with_no_flag`, `test_s002_allow_hooks_arms_and_the_row_names_the_arming_client` | `test_hook_arming.py` |
| **S-002b** install report names client + tier | **defect** | `test_s002_the_install_report_names_the_arming_client_and_the_tier` (xfail) | `test_hooks_lifecycle.py` |
| **S-002c** no TTY ⇒ never prompt, never arm (C-023) | covered (pre-existing) | `test_s002_no_tty_never_arms_and_never_asks` | `test_hook_arming.py` |
| **S-002d** project entry may restrict, never grant (B4) | covered (pre-existing + **new**) | `test_a_trust_hooks_entry_in_global_config_arms_with_no_flag`, `test_trust_hooks_false_in_the_project_config_beats_a_global_grant`; **new** `test_s011_a_clone_cannot_grant_itself_registry_trust_b4` | `test_hook_arming.py`, `test_hooks_boundary.py` |
| **S-003** project payload under `$GRIM_HOME`, workspace-keyed | covered (pre-existing) | `test_a_project_hook_arms_with_nothing_armable_in_the_workspace` | `test_hook_arming.py` |
| **S-003** *global* payload `$GRIM_HOME/hooks/<name>/` | **new** | `test_s003_a_global_hook_payload_lands_directly_under_grim_home` | `test_hooks_lifecycle.py` |
| **S-003** global install *arms* | **defect** | `test_s003_a_global_install_arms_the_hook_it_materialized` (xfail) | `test_hooks_lifecycle.py` |
| **S-004** no match ⇒ nothing spawned, no hash | covered (pre-existing) | `test_a_matcher_that_does_not_match_spawns_nothing_s004` | `test_hook_run_runtime.py` |
| **S-005** match ⇒ envelope on stdin, projected per client | covered (pre-existing) | `test_a_matched_hook_spawns_the_payload_with_the_envelope_on_stdin`, `test_the_payload_runs_from_its_payload_dir`; per-client projection by `projector.rs` unit tests (`every_shipped_pair_permits_its_own_verdict_and_reason`, `the_firing_event_is_echoed_in_its_native_spelling_on_claude_and_codex`) | `test_hook_run_runtime.py`, `src/command/hook/projector.rs` |
| **S-006** gatekeeper deny reaches the client | covered (pre-existing) | `test_a_gatekeeper_deny_reaches_the_client_as_json_never_an_exit_code_s006` | `test_hook_run_runtime.py` |
| **S-007** digest change ⇒ re-prompt (post-reversal form) | covered (pre-existing) | `test_s007_a_digest_change_cannot_arm_an_untrusted_registry_on_update` | `test_hook_arming.py` |
| **S-008** uninstall reaps row + registration + payload | covered (pre-existing) | `test_s008_uninstall_reaps_the_row_and_the_registration` | `test_hook_arming.py` |
| **S-009** grim absent/mid-upgrade ⇒ no client blocks | covered (pre-existing) + **new** | `test_a_payload_that_cannot_be_spawned_never_blocks_s009`; **new** `test_the_registered_command_exits_zero_when_the_launcher_is_unusable_b8` covers the *launcher* half the runtime test explicitly disclaims | `test_hook_run_runtime.py`, `test_hooks_boundary.py` |
| **S-010 v1** nothing armable in the working tree | covered (pre-existing) | `test_a_project_hook_arms_with_nothing_armable_in_the_workspace` | `test_hook_arming.py` |
| **S-010 hostile variant** a committed registration must not execute | **new** | `test_s010_a_committed_registration_cannot_fire_the_victims_hooks_b3` | `test_hooks_boundary.py` |
| **S-011** repo's own committed `state.json` + payload | covered (pre-existing) | `test_a_cloned_workspaces_own_committed_hook_state_must_not_arm` (2 variants) | `test_hook_arming.py` |
| **S-011 case 1 (B1)** planted `GRIM_HOME` + committed table | **new** | `test_s011_a_clone_that_plants_grim_home_arms_nothing_b1` (relative, nested-absolute), `test_s011_a_committed_table_is_never_adopted_by_an_honest_install_b1` | `test_hooks_boundary.py` |
| **S-011 case 2 (B3)** foreign registration, forged root | **new** | `test_s010_a_committed_registration_cannot_fire_the_victims_hooks_b3` (roots `global`, absolute workspace path, case-flipped token) | `test_hooks_boundary.py` |
| **S-011 case 3 (B4)** repo-granted registry trust | **new** | `test_s011_a_clone_cannot_grant_itself_registry_trust_b4` | `test_hooks_boundary.py` |
| **WP-O case 4 (B8)** guard fail-closed states | **new** | `test_the_registered_command_exits_zero_when_the_launcher_is_unusable_b8` (directory, missing interpreter) | `test_hooks_boundary.py` |
| **WP-O case 5 (B7)** `trust_hooks = false` survives `grim add` | **new** | `test_trust_hooks_false_survives_a_grim_add_b7` | `test_hooks_boundary.py` |
| **B5** bare host never grants implicitly | **new** | `test_a_bare_host_registry_entry_never_grants_hook_trust_implicitly_b5` | `test_hooks_boundary.py` |
| two-workspace approval boundary | **new** | `test_a_hook_armed_in_one_workspace_never_fires_from_another` | `test_hooks_boundary.py` |
| **S-012** config write refused with "run `grim install` to disarm" | **new** | `test_s012_turning_the_feature_flag_off_through_config_is_refused` (`set false`, `unset`) | `test_hooks_lifecycle.py` |
| **S-013** client with no hook surface ⇒ Declined, warned, zero outputs, visible in status | **new** | `test_s013_a_client_with_no_hook_surface_declines_warns_and_records_nothing` | `test_hooks_lifecycle.py` |
| **S-014** *lock* half — older binary meets a hooks-bearing lock | **not testable** | the message is emitted by the *older* binary and there is no multi-binary fixture (the plan says so itself, § "Literal old-binary behaviour"). Nothing the current binary does can change it. See the finding below — the current binary emits a bare serde field list for the analogous case | — |
| **S-014** *state* half — state written by a newer grim | **new** | `test_s014_install_state_from_a_newer_grim_names_the_version_requirement` | `test_hooks_lifecycle.py` |
| **S-015** `grim hook list` is a report command | covered (pre-existing) + **new** | `test_hook_list_is_an_ordinary_report_command_s015` (global, no hooks); **new** `test_s015_hook_list_is_a_scope_resolving_report_command` (project, hook declared and armed) | `test_hook_run_runtime.py`, `test_hooks_lifecycle.py` |
| **S-015** it reports the declared hook | **defect** | `test_s015_hook_list_reports_the_declared_hook_its_tier_and_its_arming_state` (xfail) | `test_hooks_lifecycle.py` |
| **S-016** mutator rewrite surfaced to the model | covered (pre-existing) | `test_a_mutator_rewrite_is_also_surfaced_to_the_model_s016` | `test_hook_run_runtime.py` |

Also re-measured: `test/recordings/cast_recorder.py:106-119`. Its comment sized
`grim status` at **5** columns; the hook kind's `Note` column takes it to 6. The
widest line is the *header* row (`print_table` trims the last column's trailing
padding): `5 + 2 + 10 + 2 + 6 + 2 + 109 + 2 + 9 + 2 + 4 = 153` for the demo's
`ghcr.io/grimoire-rs/skills/grim-usage` ref. The same arithmetic reproduces the
comment's previous 147 exactly, which is what validates the model. `width = 180`
is unchanged and still has headroom — but the margin is 27 cols, not 33.

## What makes each new test fail if the feature regresses

| Test | The one-line answer |
|---|---|
| `..._clone_that_plants_grim_home_arms_nothing_b1` | If `validate_grim_home` stopped refusing, the install would arm from a repo-resident root and `status`'s cause would not be `grim-home-relative` / `grim-home-in-workspace`. Registry trust is granted first so the *reported* cause is the `$GRIM_HOME` gate and not `registry-not-trusted`. |
| `..._committed_table_is_never_adopted_by_an_honest_install_b1` | Fires the victim's real registration: the honest sentinel appears (control) and the hostile one does not. If convergence ever read a table out of the workspace, the hostile sentinel appears. |
| `..._committed_registration_cannot_fire_the_victims_hooks_b3` | The control run with the honest root token fires the payload; the same command with `--root global`, `--root <abs workspace>`, or a case-flipped token fires nothing. The *only* difference between the legs is the root, so the test discriminates on exactly the B3 control. |
| `..._clone_cannot_grant_itself_registry_trust_b4` | The identical `[[registries]] trust_hooks = true` entry arms when authored globally (control) and arms nothing when authored in the repo's own `grimoire.toml`, with no prompt string on either stream. |
| `..._bare_host_..._implicitly_b5` | A bare-host global entry arms nothing; its namespaced sibling, same registry, arms. Regressing `is_bare_host` flips the first leg. |
| `..._trust_hooks_false_survives_a_grim_add_b7` | Asserts the literal `trust_hooks = false` line in global config *after* `grim add` rewrote the file, plus that `--allow-hooks` still cannot arm. The emit-only-when-true bug drops the line and the second `grim install` arms. |
| `..._registered_command_exits_zero_when_the_launcher_is_unusable_b8` | Executed proof of discrimination: with the shipped guard a directory at the launcher path exits **0**; with `exec` and no `[ -f ]` the same shell string exits **126** (measured), which on Copilot's fail-closed `preToolUse` denies the user's tool call. |
| `..._armed_in_one_workspace_never_fires_from_another` | Two workspaces arm *different* artifacts with *different* sentinels; firing A's registration produces A's sentinel only. Collapsing the per-root key produces both. |
| `..._global_hook_payload_lands_directly_under_grim_home` | Exact path assertion plus the negative that `hooks/payload/` (the project-scope shape) does not exist at global scope. |
| `..._turning_the_feature_flag_off_through_config_is_refused` | The hook is armed **first**, so exit 65 + "run `grim install` to disarm" is observed against a real armed installation; the file is asserted unchanged and the dispatch row still present, then the working route (edit + `grim install`) is asserted to disarm. |
| `..._client_with_no_hook_surface_declines_warns_and_records_nothing` | Cursor: warning naming client and artifact, `skipped` + `target: null`, `outputs: []`, cause `client-has-no-hook-surface`, no dispatch row — then Claude, same artifact and registry, arms (control). |
| `..._install_state_from_a_newer_grim_names_the_version_requirement` | Asserts a **hook** record is present in the state file first, then that the error names "newer version of grim", the version number and "upgrade", and explicitly that it is *not* a raw serde `expected one of` field list. |
| `..._hook_list_is_a_scope_resolving_report_command` | Exit 0, `items` envelope, and the six documented plain columns, in a project that actually declares and arms a hook. |

## Findings

### F-1 — Block. A global-scope install can never arm a hook.

`command/scope_resolution.rs:93` sets `workspace = paths.root()` (i.e.
`$GRIM_HOME`) for global scope. `install/hook_registrar.rs:908-925`
(`validate_grim_home`) then refuses whenever
`resolved(grim_home).starts_with(resolved(workspace))` — which `$GRIM_HOME`
satisfies against itself, unconditionally. The check's own doc explains that
cause 2 is deliberately evaluated at *both* scopes so that `grim install
--global` run from inside a repository cannot arm; what it did not account for
is that at global scope the workspace **is** `$GRIM_HOME`.

Executed evidence, `grim install --global --allow-hooks` with the feature flag
on and a global `[hooks]` declaration:

```
hook  shell-guard  .../grim-home/hooks/shell-guard  installed
WARN grim::install::hook_registrar: hooks not armed for claude: GRIM_HOME resolves
     inside this workspace, which would make an armable file repo-resident
WARN ... for copilot: ...   WARN ... for codex: ...
```

The payload is materialized at `$GRIM_HOME/hooks/shell-guard/` (S-003 holds),
but no `hooks/dispatch.json`, no `hooks/bin/grim-hook` and no registration are
written. This is the "installed but does nothing" shape the whole `not-armed`
vocabulary exists to avoid, and global is the scope `docs/src/clients.md`
describes as the only one Codex and Copilot arm at all.

Direction, not a patch (I did not touch `src/**`): `validate_grim_home` needs to
treat `ConfigScope::Global` as having no workspace to be nested inside — the
`scope` parameter it currently discards on purpose. Whoever takes it should keep
the project-scope behaviour exactly as it is, including `grim install --global`
run from inside a repository whose *project* root is what matters.

Pinned by `test_s003_a_global_install_arms_the_hook_it_materialized` (xfail,
strict).

### F-2 — Warn. `grim hook list` is still an unconditional empty report.

`src/command/hook/list.rs:53-68` returns `HookListReport::new(Vec::new())`
regardless of scope. Verified against the built binary with a declared **and
armed** project hook: `grim hook list --format json` → `{"items": []}` while
`$GRIM_HOME/hooks/dispatch.json` carries the row. The stub's own REMOVAL TRIGGER
says to replace it "once a hook can be installed", and that premise is false
since WP-J2/WP-R landed — so the stub is stale, not pending.
`.claude/rules/subsystem-cli-commands.md` already carries this ⚠, so the rule
and the code agree; the gap is real work, not documentation drift.

I checked the binary before writing anything against it, as instructed. No
concurrent package had fixed it as of base `7bbc348`.

Pinned by `test_s015_hook_list_reports_the_declared_hook_its_tier_and_its_arming_state`
(xfail, strict).

### F-3 — Warn. S-002's second half is unimplemented.

An armed hook's install-report row is the generic
`{kind, name, target, status}` — verified by execution:

```json
{"items": [{"kind": "hook", "name": "shell-guard",
            "target": ".../hooks/payload/<sha>/shell-guard", "status": "installed"}]}
```

It names neither the arming client nor the tier, so the one moment a user is
told that a published artifact just gained the ability to run on their tool
calls reads only "installed". `grim status` already carries both through
`HookArming`, so the data exists. The plan leaves the owner ambiguous — WP-R's
row says "*possibly* `src/api/install_report.rs` (S-002's second half, **if**
assigned here rather than WP-M)" — and neither package built it.

Pinned by `test_s002_the_install_report_names_the_arming_client_and_the_tier`
(xfail, strict), asserted on *content* (the strings `claude` and `observer`
appearing in the row) rather than on invented field names, so any reasonable
additive implementation satisfies it.

### F-4 — Suggest. The `client-has-no-hook-surface` message overstates.

Its text is "…there is nothing to arm; **the payload is installed** and every
other client is unaffected". When the resolved client set contains *only*
surface-less clients, the payload is **not** installed: the install report is
`{"target": null, "status": "skipped"}` and the state record's `outputs` is
`[]` (both verified by execution). The message is accurate in the mixed case
and wrong in the only-declined case. Wording only — no behaviour change.

### F-5 — Not a defect, recorded so it is not re-found. S-014's lock half is structurally untestable.

The current binary rejects an unknown lock section with a bare serde list:

```
grimoire.lock: invalid TOML: TOML parse error at line 16, column 3
unknown field `unknown_future_kind`, expected one of `metadata`, `skill`,
`rule`, `agent`, `mcp`, `hook`, `bundle`
```

That is the shape S-014 calls "a bare TOML parse failure" — but the message an
*older* grim emits when it meets a `[[hook]]` section comes out of the **older
binary**, which no change to this one can improve, and there is no multi-binary
fixture. The testable and implemented half is the install-state read path
(`install_state.rs:681-694`), which the plan's own Principle-9 row extends
S-014 to; that is what `test_s014_...` covers.

## Notes on method

- Every "nothing executed" assertion in `test_hooks_boundary.py` is backed by a
  sentinel file the published payload `touch`es *outside* the workspace, and
  every one of them sits in a test function whose control leg fires that same
  sentinel first. Exit 0 is never used as evidence that a refusal happened —
  every refusal on this path exits 0 by design (I3).
- The planted dispatch tables use the **real** schema
  (`{"schema": 1, "roots": {<token>: {"root": …, "hooks": […]}}}`), not the
  pre-freeze placeholder in `conftest.py`'s `hostile_hook_clone` factory — that
  fixture's own docstring asks for this once the format froze. I did not change
  the fixture; `test_harness_fixtures.py` pins its current shape and the two new
  files build their plants inline, which is also what keeps each attack readable
  in isolation (DAMP).
- `test/src/helpers.py` was **not** touched: `write_config`'s `hooks=` and
  `options=` parameters covered every fixture needed.
- The registry-trust fixtures are stated explicitly in each test, because
  labelling a registry in *global* config **is** the consent act (B4) while a
  bare host is not (B5), and getting that backwards is what made two earlier
  workers read the implicit grant as a laundering hole.
