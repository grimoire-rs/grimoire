# Round 1 review — tests & code quality (`hex/hooks-artifact-kind`, tip `a5399fd`)

Reviewer: hex reviewer, focus `quality`, test-adequacy emphasis.
Scope: `git diff main...hex/hooks-artifact-kind -- src/ test/` (~28.8k added lines).
Standards: `quality-core.md`, `quality-rust.md`, `quality-rust-errors.md`,
`quality-rust-exit_codes.md`, `subsystem-tests.md`, `quality-python.md`.

Status: **COMPLETE**. Two sections, each Block / Warn / Suggest, most severe first.

> **Line-number note.** The review was run against tip `a5399fd` as briefed. While
> it was in progress the branch advanced to `01ce10f` (`c435209` "bound the child
> wait, and correct three stale claims", plus two `.agents/` records). `c435209`
> touched `src/install/hook_dispatch.rs` (net zero lines — a doc block moved
> between two adjacent functions), `src/command/hook/pipeline.rs` (+28 after
> `:840`) and `test/tests/test_hook_run_runtime.py` (+1 at `:32`, +47 appended).
> **Every citation in this report has been re-verified against the current tree
> and points at the right line there.** None of the 20 findings below is addressed
> by `c435209`; I re-read every cited site after it landed.

---

## Tests

### Block

**T-B1 — `src/hook/audit.rs:556–642` (impl) / `:645–738` (test mod): the audit
trail's rotation and its record cap have no test at all.**

The module doc (`src/hook/audit.rs:5`) states the trail is "sanitized on the way
in, capped per record, and rotated", and names the three mechanisms explicitly at
`:25–26` (`sanitize`, `MAX_RECORD_BYTES`, `MAX_LOG_BYTES`). The `#[cfg(test)]` mod
contains exactly three tests — `a_batch_appends_one_whole_line_per_record`,
`an_empty_batch_creates_no_trail`, `writable_creates_what_is_missing_and_refuses_what_cannot_be_opened`
— and **none of them exercises**:

* `rotate_if_needed` (`:564`) — the `MAX_LOG_BYTES` threshold, the rename to
  `hooks.jsonl.1`, or the documented "single retained generation, replacing any
  previous generation" (`:556`). Nothing anywhere pins that a second rotation
  discards the first generation, so the module's stated bound of
  `2 * MAX_LOG_BYTES` (`:113–116`, `:576`) is unverified.
* `rotated_path` (`:588`) — the suffix-append rule the doc distinguishes from a
  stem-replace (`hooks.jsonl.1`, never `hooks.1`).
* `capped_line` (`:609`) — the four-step elision ladder, `ELIDED`, or the
  "truncate, not drop" property. I verified the ladder is in fact sound
  (`correlation_id` is grim-generated and bounded — `run.rs:383` yields
  `<pid-hex>-<nanos-hex>`, so the final unconditional `encode` really is bounded);
  that soundness is asserted by nobody.
* `sanitize` (`:408`) — a `pub` function on the CWE-117 path, called on every
  string of every record (`:321–334`).

Grep confirms the gap is not covered elsewhere: `grep -rn
"hook_audit\|sanitiz\|ELIDED\|rotat" test/tests/` yields only two path helpers
(`test_example_hooks.py:306`, `test_hook_run_runtime.py:191`) and no assertion on
rotation, caps or escaping.

**Remediation** (all pure, all cheap, all in the existing `mod tests`):
1. `rotate_if_needed`: write `MAX_LOG_BYTES` bytes, append one record, assert
   `hooks.jsonl.1` exists and the live trail holds only the new record; repeat and
   assert generation 1 was *replaced*, not accumulated (no `.2`).
2. `capped_line`: build a record whose `changed_fields` and `hook_id` are each
   over `MAX_RECORD_BYTES`, assert the encoded line is `<= MAX_RECORD_BYTES` at
   every rung and that the elided fields read exactly `ELIDED` while
   `hook_id`/`outcome`/`verdict` survive on the earlier rungs (the "truncate, not
   drop" property).
3. `sanitize`: assert `\u{001b}`, `\u{0000}`, `\u{007f}` and a C1 byte each become
   the `\u{XXXX}` literal and that a plain UTF-8 string is returned unchanged.

**actionable**

### Warn

**T-W1 — `test/tests/conftest.py:98–183` + `test/tests/test_harness_fixtures.py:119–169`:
the `hostile_hook_clone` fixture plants a dispatch table in a schema that no
longer exists, is used by nothing but its own self-test, and its one grim-facing
test has no positive control.**

Three defects compounding, all instances of the branch's own checklist item 3
("a fixture that accidentally grants what it meant to withhold" / item 2 "green
because nothing happened at all"):

1. **Stale schema.** The fixture's docstring says "The hooks dispatch-table
   schema is not frozen yet (WP-I)" (`conftest.py:127`) and instructs a future
   author to "pass a **repo-local** `dispatch_relative` that *mirrors*" the real
   shape once frozen. It *is* frozen on this branch:
   `src/install/hook_dispatch.rs:120` `DISPATCH_SCHEMA: u32 = 1`, and the wire
   form is `{"schema":1,"roots":{<32-hex token>:{"root":…,"hooks":[…]}}}`. The
   fixture plants `{<absolute workspace path>: [{"client","event","matcher","command"}]}`
   (`conftest.py:170–183`) — no `schema` key, path-keyed instead of token-keyed,
   and rows in a shape `DispatchEntry` cannot deserialize. A read path that *did*
   read this file would take the `DispatchDegrade` branch (`hook_dispatch.rs:120–127`:
   an unrecognized schema is "an empty table, one log line, exit 0"), spawn
   nothing, and every `assert not sentinel.exists()` would pass regardless. The
   fixture therefore cannot detect the defect it exists for.
2. **Unused.** `grep -rn "hostile_hook_clone" test/` finds consumers only in
   `test_harness_fixtures.py`. The real boundary suite builds its own plant inline
   and — correctly — in the **real** schema (`test_hooks_boundary.py:193–219`,
   `:270–296`). So ~85 lines of conftest and two of its three self-tests are dead
   test infrastructure with a misleading docstring.
3. **No positive control on the one grim-facing test.**
   `test_hostile_clone_today_grim_status_never_touches_the_planted_payload`
   (`test_harness_fixtures.py:154–169`) runs `grim init` + `grim status` in the
   clone and asserts `not clone.sentinel.exists()`. Nothing is declared, locked,
   installed or armed in that test, so no build of grim could ever touch the
   payload; the assertion is satisfied by a binary with the hooks feature deleted.
   Its docstring even says so out loud — "a `grim` build with **no hook support at
   all**" and "WP-O extends this once `grim hook run` exists" — both of which are
   false on this branch: hook support ships here, and WP-O did not extend this
   test, it wrote a separate one.

**Remediation** — pick one, do not leave it as is: either (a) delete the fixture,
its dataclass and the three self-tests, and note in `test_hooks_boundary.py` that
the plant is built inline in the frozen schema on purpose; or (b) re-point the
fixture at the frozen schema (`schema`/`roots`/token key/`handler.argv`/
`payload_dir`), have `test_hooks_boundary.py` consume it instead of its two inline
plants, and give the surviving grim-facing test the arm-then-fire control that
`test_hooks_boundary.py:309–312` already demonstrates. (a) is the smaller change
and loses nothing — the inline plants are strictly better.
**actionable**

**T-W2 — `test/tests/test_hook_decline_dispatch.py:210–219`: the inverted P-1
regression pin has no positive control, in a file whose sibling suite mandates
one.**

The assertions were genuinely inverted, not relaxed — I diffed `05b6d20` and
confirmed every one of `state == "not-armed"`, `arming == [("claude","not-registered")]`,
`[r["id"] for r in rows] == ["watch"]`, `not marker.exists()` and
`"updatedInput" not in result.stdout` is the negation of the pre-fix assertion.
The gap is the control. Step 4's `assert not marker.exists()` is the "declined
mutator never ran" negative, and the only payload with an observable effect in
this artifact is the *declined* one: `watch.sh` is
`#!/bin/sh\ncat > /dev/null\nprintf '%s' '{}'\n` (`:136`), so nothing in this test
proves the dispatcher spawned anything at all. A build whose `spawn` is a no-op
passes step 4 — and the file's own comment concedes the stdout is empty ("the
dispatcher emits no document at all here", `:216`). `test_hooks_boundary.py`'s
module docstring states the standard this file falls short of: "⛔ **Every
negative here is paired with an executed positive control**" (`:19–22`).

**Remediation**: give `watch.sh` a sentinel — `touch '{watch_marker}'` alongside
its `printf` — and assert `watch_marker.exists()` immediately before the
`not marker.exists()` assertion, with a `POSITIVE CONTROL FAILED` message, exactly
as `test_hooks_boundary.py:311` does. One line of fixture, one assertion.
**actionable**

**T-W3 — `src/lock/grimoire_lock.rs:554–570`: `iter_artifacts_chains_all_kinds_in_order`
names "all kinds" and pins three of five.**

The fixture populates `skills`, `rules`, `agents` and leaves `mcp: vec![]`,
`hooks: vec![]`, then asserts
`kinds == vec![Skill, Rule, Agent]`. Deleting `.chain(self.mcp.iter())` or
`.chain(self.hooks.iter())` from `iter_artifacts` (`:193–199`) leaves this test
green — which is precisely the property the method's own doc claims it has ("The
single chaining seam: consumers that walk 'all locked artifacts' go through here
so a future kind cannot be forgotten", `:184–187`). This is the name-over-promises
pattern the brief names as checklist item 1.

Partial cover exists but does not close it:
`hook_array_round_trips_and_is_omitted_when_empty` (`:619–655`) asserts
`iter_artifacts` yields **one** `Hook` (a count, not a position), and
`mcp_array_round_trips_and_is_omitted_when_empty` asserts nothing about
`iter_artifacts` at all.

**Remediation**: populate all five lists in the existing fixture and assert the
full order `[Skill, Rule, Agent, Mcp, Hook]`. Two added lines; it then also fails
if a sixth kind is chained out of order.
**actionable**

**T-W4 — `test/tests/test_hook_run_runtime.py:899–916`: a stale test whose only
assertion is a tautology, and whose docstring says the weakness is unavoidable
when it no longer is.**

`test_hook_list_is_an_ordinary_report_command_s015` states: "**Weak by
necessity:** nothing can install a hook until the installer's ``Hook`` branch
lands, so an empty ``items`` array is the correct answer today and this asserts
only the envelope and the exit code." That premise expired on this branch — the
installer's `Hook` branch is at `src/install/installer.rs:456` onward and
`grim hook list` is populated (`9f0a02f`). The surviving assertion is
`isinstance(report["items"], list)`, which passes for a build with the hook kind
deleted, and the same scenario is now pinned properly five times over in
`test/tests/test_hook_list.py:107–251` plus
`test/tests/test_hooks_lifecycle.py:339`.

**Remediation**: delete the test (its coverage is superseded), or keep only the
6-column plain-table header assertion and replace the docstring — do not leave a
docstring telling the next reader that a tautology is the best available
assertion.
**actionable**

**T-W5 — `converge_clients` is never exercised with more than one client, nor at
global scope, by any test.**

Every one of the 17 unit-test call sites (`src/install/hook_registrar.rs:1751,
1781, 1813, 1860, 1927, 1938, 1966, 1972, 1998, 2011, 2084, 2105, 2165, 2437,
2507, 2559, 2604`) passes `ConfigScope::Project`, and `hook_clients(Project)`
resolves to Claude alone — Codex and Copilot are global-scope-only (amendment
A1, `installer.rs:1192–1196`). On the acceptance side the two global tests
(`test/tests/test_hooks_lifecycle.py:91` and `:121`) create only `.claude`
(`:102`, `:133`) and `:139` asserts *exactly one* dispatch row, so extending
them is not possible without a new test.

The consequence is precise: `converge_clients`'s two headline properties — the
union written **once** over every hook-capable client (the F-1 defect its doc at
`:266–274` describes, "the last client's write erase[s] every earlier client's
rows"), and the P-1 step order with more than one client contributing — are
verified only through the components (`desired_entries` + `register_desired` +
`union_of`, `:2214`/`:2247`, which pass `surface: None` and so write no client
config at all) and through a hand-authored table on the runtime side
(`test_hook_run_runtime.py:429`). The orchestrator that owns the ordering, five
early returns and the launcher/table write sequencing runs in exactly one client
configuration.

**Remediation** — one test, either level:
* unit: `converge_clients(&state, home, ConfigScope::Global, &roots, &arming_policy())`
  with `hook_record(..., RootScope::Global, &["claude", "codex"])`, asserting two
  rows with distinct `client` in one `converge_root` write, both clients'
  surfaces written, and both reaped by a follow-up call with an empty state;
* or acceptance at global scope with `CODEX_HOME` pointed into `tmp_path` (the
  idiom `test/tests/test_clients.py` already uses), asserting two rows, a
  grim-marked registration in each client's own file, and both gone after
  `grim uninstall`.
**actionable**

### Suggest

**T-S1 — `src/install/hook_registrar.rs:1531–1543`: the global-scope placeholder
carve-out that `validate_grim_home` exists to make work is not unit-tested.**

`a_grim_home_inside_the_workspace_refuses_to_arm_at_either_scope` iterates
`[Project, Global]` and asserts **refusal** in both, which correctly pins the
"a real workspace at global scope still refuses" half the comment at `:1051–1055`
promises ("the check still fires if a caller ever passes a real workspace at
global scope"). The other half — `workspace == grim_home` at global scope must
return `Ok(())`, the WP-O defect where `grim install --global` could never arm
anything — has no unit test here. It is pinned at acceptance level by
`test/tests/test_hooks_lifecycle.py:121` (`test_s003_a_global_install_arms_the_hook_it_materialized`),
so this is a locality complaint, not a coverage hole: a reader "simplifying" the

**T-S2 — `src/command/hook/argv.rs`: 188 lines, zero unit tests, and one refusal
branch (`ArgvRefusal::EmptyRoot`) exercised nowhere.**

`argv.rs` holds what `run.rs:43` calls "the only untrusted input grim" reads:
`validate` (`:146`) with four refusal branches and the B1 absoluteness gate, plus
`canonical_event` (`:186`). It is the only file under `src/command/hook/` with no
`#[cfg(test)]` block. Three of its four branches are reached through a
subprocess: `TableNotAbsolute` (`test_hook_run_runtime.py:340`), `UnknownEvent`
and `EmptyClient` (`:466–488`). `grep -rn 'EmptyRoot|"root", ""' src/ test/`
finds no exerciser at all — `test_no_invocation_shape_ever_exits_non_zero_i3`
(`:863`) passes `root: "not-a-token"`, which is the *unknown*-root path, not the
empty one. `canonical_event`'s documented deliberate `None` for codex's native
`PermissionRequest` (`:183–185`) is only reached as the `UnknownEvent` case, so
the distinction the doc draws is not asserted.

`validate` and `canonical_event` are pure functions over a `RunArgs` struct with
no I/O — `subsystem-tests.md`'s placement rule puts them in a unit test, and
routing them through a real process is both slower and coarser. **Remediation**:
add a `mod tests` to `argv.rs` covering all four refusals plus the two `Ok`
shapes (roughly 25 lines), and keep the acceptance sweep as the end-to-end
regression guard it already says it is.
**actionable**

**T-S3 — coverage that is present, recorded here so it is not re-requested.**
Checked and found genuinely pinned, contrary to the brief's suspicions:
* reap / uninstall — `test_hook_arming.py:370`, `test_example_hooks.py:357`,
  `hook_registrar.rs:1938` and `:2011`.
* bundle-delivered hooks — `test_bundle_hook_members.py` (10 tests, including
  `:277` "not armed by membership alone" and `:315` eviction).
* Decision O's mutator ordering — `src/command/hook/pipeline.rs:1139`, `:1191`,
  `:1249`, `:1383`, `:1406`, `:1417` (all four parts, per part).
* the multi-client dispatch union — `hook_registrar.rs:2214` and `:2247` at the
  component level, `test_hook_run_runtime.py:390` and `:429` at the runtime
  level. **But** see T-W5.
* the git-exclude hygiene — `hook_registrar.rs:2287–2411`; no acceptance test
  reaches it because `project_dir` is not a git worktree, which is fine.
* `grim status` vs `grim hook list` agreement — `test_hook_list.py:191`.
**deferred** (nothing to do)

---
## Code quality

### Block

**Q-B1 — the "does this client have a writable hook surface at this scope"
predicate is spelled four times, the codebase's own doc says to collapse it, and
the doc's stated reason for the delay expired on this branch.**

`src/command/status.rs:886–902`, verbatim:

> **Owed, and flagged rather than silently duplicated:** collapse this to
> `client_supports_kind(client, ArtifactKind::Hook, …)` **in the same change
> that adds that arm (WP-J2)**. Two spellings of one predicate is how the
> browse filter and the TUI tree came to disagree about a row.

WP-J2 landed on this branch — `client_supports_kind`'s `Hook` arm is at
`src/install/installer.rs:1208`. The collapse was not done, so the branch ships
the exact defect its own comment names as the reason for the note. Worse, the
sentence above it is now false: *"That function's `Hook` arm is WP-J2's to add,
and **until it lands** its catch-all answers `kind_support(Hook) != Declined`,
which is `true` for all 18 clients"* — the catch-all no longer answers `Hook` at
all. A reader who trusts that paragraph concludes the duplication is still
necessary.

The four spellings, all present at tip:

| # | Site | Expression |
|---|---|---|
| 1 | `src/install/installer.rs:1208` — `client_supports_kind` (canonical) | `hook_surface().is_some() && kind_surface(kind, scope)` |
| 2 | `src/command/status.rs:901` — `HookArmingInputs::client_has_hook_surface` | `hook_surface().is_some() && kind_surface(ArtifactKind::Hook, self.scope)` — byte-identical |
| 3 | `src/install/hook_registrar.rs:485–490` — `hook_clients` | `hook_surface().is_some() && kind_surface(ArtifactKind::Hook, scope)` — byte-identical; its own doc at `:480–481` admits the copy ("Spelled through the same two seams `installer::client_supports_kind`'s `Hook` arm uses") |
| 4 | `src/install/path_anchor.rs:994` — `is_declined_global_pair` | `hook_surface().is_none()` — **first conjunct only, scope-blind**, while its doc at `:986–991` claims "this predicate must agree with `client_supports_kind`'s `Hook` arm (`hook_surface().is_some() && kind_surface(kind, scope)`)". It agrees on half. Harmless today because it is only asked at `ConfigScope::Global` (`:734`), where the second conjunct is `true` for all three hook clients — but the doc states an agreement the code does not have. |

The unit test meant to hold these together does not.
`src/install/installer.rs:5944–5956` computes
`let expected = client.vendor().hook_surface().is_some() && client.vendor().kind_surface(ArtifactKind::Hook, scope);`
— a **fifth** spelling, inside the assertion. It is a tautology over the wrapper:
change the rule and both sides move together, so it can only catch a typo in
`client_supports_kind`'s delegation, never a change of meaning. (The four literal
assertions that follow at `:5959` onward do pin the three shipped clients; that
half has value.)

**Remediation** — one seam, three delegations, one honest test:

1. Add a default method to `Vendor` beside `hook_surface`, e.g.
   `fn hooks_at(&self, scope: ConfigScope) -> bool { self.hook_surface().is_some() && self.kind_surface(ArtifactKind::Hook, scope) }`.
   That is the right home because both conjuncts are vendor seams, and because
   `HookArmingInputs` carries no `workspace` field (`src/command/status.rs:766–822`)
   — so delegating to `client_supports_kind` directly would force a synthetic path
   argument that its `Hook` arm ignores anyway.
2. Point `installer.rs:1208`, `status.rs:901` and `hook_registrar.rs:490` at it.
   Delete the "Owed" paragraph and the expired "until it lands" premise from
   `status.rs:888–899`.
3. Correct `path_anchor.rs:986–991` to say it deliberately asks only the first
   conjunct and why (it is asked only at global scope) — or route it through
   `hooks_at(ConfigScope::Global)`.
4. Replace `installer.rs:5948–5949`'s re-derivation with a fixed expected table:
   `(Claude, Project, true)`, `(Claude, Global, true)`, `(Codex, Project, false)`,
   `(Codex, Global, true)`, `(Copilot, Project, false)`, `(Copilot, Global, true)`,
   and `false` for the other 15 clients at both scopes. That version fails when
   the *rule* changes, which is the point.

**actionable**

### Warn

**Q-W1 — `src/install/hook_registrar.rs:203–204` and `:252–254`: two doc claims
about `HookSync` that the code contradicts, over a return value no caller reads.**

* `HookSync`'s type doc (`:203–204`) calls it "the value `sync_config` logs and
  `grim status` reads". Neither holds. `converge_clients`'s own doc (`:256–268`)
  explains at length why convergence is deliberately **not** called from
  `Vendor::sync_config`; and `grim status` derives hook state from
  `HookArmingInputs` + `arming_refusal` + the dispatch table
  (`src/command/status.rs:660–745`), never from a `HookSync`.
  `grep -rn "HookSync::" src/ | grep -v hook_registrar` returns nothing — the enum
  is constructed and matched only inside its own module and its tests.
* `converge_clients`'s doc (`:253–254`) says it "Returns one outcome per client,
  **already logged**, so a caller is a single statement." Three of its five
  bulk-return paths never call `log_sync`: the root-token failure (`:378–380`),
  the launcher-write failure (`:414–416`), and the dispatch-table I/O failure
  (`:441–443`). The refusal path (`:346–354`) and the `DispatchLocked` path
  (`:427–437`) do.
* All three callers discard the return value — `src/install/installer.rs:415`,
  `src/command/update.rs:333`, `src/command/uninstall.rs:170` — and the install
  report's arming column instead re-reads the table
  (`src/command/install.rs:436–465`, `armed_after_convergence`). That is the right
  authority to read, so the fix is the docs, not the design.

A substantive point inside the same three paths: they report `HookSync::NoHooks`,
whose own doc (`:213–215`) defines it as "No hook is recorded for this client at
this scope, and nothing grim-owned was found to reap — the overwhelmingly common
case", and `log_sync` (`:245`) emits every non-refusal outcome at `debug`. So a
genuine launcher-write or table-I/O failure is reported through the variant
meaning "nothing to do". The user does still get a `tracing::warn!` at each site,
and `grim status` still reads `not-armed`/`not-registered` through
`status::merge_not_registered`, so this is a naming defect rather than a silent
failure.

**Remediation**: (a) rewrite `HookSync`'s doc to say what it is — the module's own
per-client outcome value, consumed by `log_sync` and by the unit tests, with
`grim status` deriving its answer independently from the table; (b) either call
`log_sync` on the three paths or drop "already logged" from `converge_clients`'s
doc; (c) consider a `HookSync::Failed` variant so "nothing to do" and "arming
failed" stop being one value — additive, so Principle 9 permits it.
**actionable**

**Q-W2 — three new library error types hand-roll `Display`/`Error` where
`thiserror` is the house default and the format logic is one `write!` per
variant.**

`quality-rust-errors.md:66`: "Error types without `#[derive(thiserror::Error)]`:
manual `Display` impls OK **only when format logic too complex** for
`#[error(...)]`; new types default to thiserror." The branch adds four error types
and only one follows it (`oci::hook::HookError`, `src/oci/hook.rs:1363`, which
does it correctly including `#[source]` on the wrapping variant).

* `src/install/hook_dispatch.rs:771–796` — `DispatchError`: two variants, hand
  `Display` (`:781`) plus hand `source()` (`:790`). Directly expressible as
  `#[error("another grim install holds the dispatch table lock")] Locked` and
  `#[error("dispatch table I/O failure: {0}")] Io(#[source] io::Error)`.
* `src/install/hook_launcher.rs:443–469` — `LauncherError`: same shape, same
  two-variant triviality.
* `src/command/hook/projector.rs:82–147` — `ProjectionError`: three variants, each
  a single `write!` over named fields, plus
  `impl std::error::Error for ProjectionError {}` (`:147`) with **no `source()`** —
  so the day a variant wraps an error the chain breaks silently.
  `#[error("client '{client}' hosts no hook at {event}")]` and its two siblings
  cover the current text verbatim.

Not flagged, for the record: `ArmRefusal`, `CommandRefusal`, `ArgvRefusal`,
`EnvelopeError`, `HookDecline`, `Decision`, `RootToken`, `CanonicalEvent` and
`HookTier` are `reason()`-style value types with a `Display` and no `Error` impl —
an established pattern here and correct.

**Remediation**: convert the three to `#[derive(Debug, thiserror::Error)]` with
`#[source]` on the wrapping variants. Messages are already lowercase and
period-free, so no text changes. Mechanical and behaviour-preserving.
**actionable**

**Q-W3 — `src/hook/policy.rs:96–110`: `HookPolicy::new` takes two adjacent
same-typed `bool`s of different meaning, and one call site passes a bare `false`.**

`pub fn new(feature_enabled: bool, allow_hooks: bool, interactivity: Interactivity, registries: …)`.
Transposing the two compiles and yields a plausible policy in either direction
("gated but `--allow-hooks`" vs "enabled but not allowed") — and both bools gate
arming, the sharpest thing this feature decides. `quality-core.md`'s Warn tier
names exactly this ("Boolean parameters where enum/literal type clearer"). The
propagation makes it worse at the call sites:
`src/command/hook_consent.rs:214` re-exposes the flag as
`resolve_without_consent(ctx, scope, allow_hooks: bool)`, and
`src/command/uninstall.rs:169` calls it as
`resolve_without_consent(ctx, &scope, false)` — a bare literal whose meaning is
recoverable only from the six-line comment above it.

**Remediation**: a one-field enum for the flag that travels
(`enum AllowHooks { Granted, Withheld }`), threaded through
`resolve_without_consent` and `HookPolicy::new`. `uninstall.rs:169` then reads
`AllowHooks::Withheld`, which is that whole comment in one token. The other three
call sites (`hook_consent.rs:222`, `src/install/target.rs:570`, and the test
helper at `policy.rs:279`) are a mechanical update.
**actionable**

**Q-W4 — `src/install/hook_registrar.rs:700–723` gates a hardcoded
Claude-specific write on a vendor-generic condition.**

`sync_for_state` runs the git-exclude hygiene for **any** vendor whose
`hook_surface() == Some(HookSurface::SpliceConfig)` at project scope (`:701`), but
the work it does is Claude-specific by literal: `ensure_settings_local_excluded` /
`drop_settings_local_exclude` (`:1392`, `:1486`) hardcode
`CLAUDE_LOCAL_SETTINGS = ".claude/settings.local.json"` (`:102`) at five points
(`:1396`, `:1402`, `:1412`, `:1494`, `:1502`), and the `AlreadyTracked` warning
text (`:717`) names Claude's file. Claude is the only `SpliceConfig` vendor today
(`src/install/vendor_claude.rs:229`; Codex and Copilot are `OwnFile` —
`vendor_codex.rs:142`, `vendor_copilot.rs:113`), so the mismatch is inert. But a
second `SpliceConfig` vendor silently gets Claude's exclude line written on its
behalf and Claude's filename in its warning, and neither the type system nor a
test would say so.

The path is also a second spelling of a value already in hand: `sync_for_state`
receives `surface: Option<&Path>` (`:675`), which *is* the vendor's own
`hook_config_path(workspace, scope)` result.

**Remediation**: pass the workspace-relative form of `surface` into the two
helpers and stop using `CLAUDE_LOCAL_SETTINGS` as a write source. The helpers
become vendor-generic and the gate stops over-promising. (Lands naturally with
Q-W5.)
**actionable**

**Q-W5 — `src/install/hook_registrar.rs` mixes hook convergence with git plumbing
in one 1513-line implementation.**

Lines `1359–1514` (`ExcludeOutcome`, `ensure_settings_local_excluded`,
`git_info_dir`, `exclude_lines`, `is_tracked`, `drop_settings_local_exclude`) plus
their tests at `2287–2411` are `.git/info/exclude` manipulation and `git ls-files`
shelling — a concern with nothing to do with hook registration, dispatch tables or
vendor surfaces. `quality-core.md`'s god-module criterion is "spanning unrelated
concerns", which this meets; the rest of the file is genuinely one concern (arming)
and is fine at its size.

`grep -rn "ensure_settings_local_excluded\|drop_settings_local_exclude" src/` shows
the only consumer is `sync_for_state` (`:702`, `:704`), so the extraction is free.

**Remediation**: move the six items and their four tests into
`src/install/git_exclude.rs`, entry points `pub(crate)`, taking the
workspace-relative path to exclude as a parameter (which also lands Q-W4).
`hook_registrar.rs` drops ~280 lines and stops being two modules in one file.
**actionable**

### Suggest

**Q-S1 — `src/command/add.rs:525–529` is unreachable dead code with a comment
implying otherwise.** The path branch's `kind` can never be `ArtifactKind::Hook`:
`Some("hook")` falls into the `Some(other) =>` arm at `:490–493` and errors with
"path sources are not supported for hook artifacts". So
`if kind == ArtifactKind::Hook && is_reserved_binding_name(&name)` at `:525` cannot
fire, and the comment above it ("a dev install names a binding the same way a
registry one does, and the payload directory is derived from it identically",
`:517–519`) reads as though a path-sourced hook exists. The registry branch's copy
at `:246` is the live one. Either delete the block or say plainly that it is a
defensive guard against a future loosening of the path-source rejection at
`src/config/project_config.rs:210`. **actionable**

**Q-S2 — `src/install/hook_registrar.rs:102` and `:114`: two `pub` constants whose
documented consumer does not exist.** `GIT_EXCLUDE_RELATIVE`'s doc says it is "for
messages and for `grim status`";
`grep -rn "CLAUDE_LOCAL_SETTINGS\|GIT_EXCLUDE_RELATIVE" src/` finds no use outside
`hook_registrar.rs`. Both should be private (or `pub(crate)`), and the
`grim status` clause dropped until something reads it. **actionable**

**Q-S3 — `src/command/hook.rs:150–155`: the source-level import ban stops scanning
at the *first* `#[cfg(test)]`, which is a false-pass direction.** `code_lines` does
`source.split_once("#[cfg(test)]")` and scans only the prefix. Today every runtime
file has exactly one occurrence, at the end (`envelope.rs:479`, `pipeline.rs:993`,
`projector.rs:443`, `run.rs:476`; `argv.rs` has none), so the guard is sound — but
a single `#[cfg(test)] use …` or a `#[cfg(test)] fn` placed mid-file would silently
unscan everything after it, and the module doc (`:33–37`) presents this test as the
structural proof of C-007. One assertion closes it: assert
`source.matches("#[cfg(test)]").count() <= 1` per file, or use `rsplit_once`. The
same file already applies exactly this discipline to a neighbouring risk
(`every_declared_runtime_module_is_checked`, `:230–250`). **actionable**

**Q-S4 — `.claude/rules/subsystem-file-structure.md:154` undercounts the
reserved-name checks.** It says `RESERVED_ARTIFACT_NAMES` "is checked in three
places" and enumerates `HookManifest::validate`, `installer::install_one` and
`grim add`. There are four `is_reserved_binding_name` call sites
(`src/command/add.rs:246`, `add.rs:525`, `src/install/installer.rs:456`,
`src/install/hook_registrar.rs:1206`) plus the manifest-name check at
`src/oci/hook.rs:669` — and the rules file omits the arming seam entirely.
`src/oci/hook.rs:175–178` gets it right ("the four sites that ask it … `grim add`'s
two paths, the installer's pre-materialization gate, and the arming seam").
Outside the `src/` + `test/` scope I was given, so flagged for whoever owns the
docs pass rather than fixed here. **deferred**

**Q-S5 — clean passes worth recording, so they are not re-audited.**

* **`# Errors` sections**: I scanned every `pub fn` returning `Result` /
  `io::Result` / `anyhow::Result` across `hook_registrar.rs`, `hook_dispatch.rs`,
  `hook_launcher.rs`, `hook/audit.rs`, `hook/trust.rs`, `hook/policy.rs`,
  `oci/hook.rs`, and `command/hook/{argv,envelope,pipeline,projector}.rs` —
  **zero** missing an `# Errors` section.
* **The dispatcher genuinely cannot panic.** No `unwrap()`, `expect(`, `panic!`,
  `unreachable!`, `todo!`, `[0]` index or `assert!` appears in the production half
  of any of the five runtime files (`argv`, `envelope`, `pipeline`, `projector`,
  `run`). `run.rs:140–141`'s "Never returns an error and never panics" is a claim
  the code keeps.
* **No boolean-parameter smell outside Q-W3**; no `PathBuf` parameter that should be
  `&Path` (`AuditLog::at(PathBuf)` at `hook/audit.rs:440` stores it); no
  stringly-typed parameter where an enum exists — `DispatchEntry::client` is
  deliberately `String` and `run.rs:422–424` states why (an unrecognized client
  must match no row rather than fail to parse).
* **The trust decision really is one source.** `hook/policy.rs:186` →
  `trust::arming` (`trust.rs:363`) → `trust::decide` (`:308`) → `trust::grants`
  (`:406`); and `status::hook_arming` (`status.rs:687`) calls `trust::decide`
  directly. No second normalization of a registry locator exists.
* **`ArtifactKind` fan-outs are collapsed.** `status_badge::find_by_repo`
  (`:150–163`) now goes through `GrimoireLock::iter_artifacts`, and
  `every_locked_kind_is_findable_by_repo` (`:216–252`) iterates
  `ArtifactKind::ALL` with a total `match`, so it fails for whichever kind a
  future edit drops. `effective_set.rs:57–62`, `resolver.rs:225–231` and
  `config/hash.rs:61–68` each gained their `Hook` arm. `oci/artifact_kind.rs:71–107`
  derives all three reverse lookups from one `ALL` array anchored by a total
  `all_index` match, and its doc is candid about what that does *not* guarantee.
* **`unreachable!` audit.** `src/skill/local_pack.rs:52`'s `Hook` arm is a genuine
  panic site; I verified all three guards its comment cites, independently, and all
  three hold: `src/config/project_config.rs:210` (`PathValues::Rejected`, the
  structural one the comment omits), `src/command/add.rs:490–493`, and
  `src/command/install.rs:241`/`:329`. It matches the pre-existing `Bundle`/`Mcp`
  arms in the same match, so it is house style rather than a new risk.

**deferred** (nothing to do)

---

## Verification log

**Doc-comment claims sampled and verified against the code: 25. Eight were false
or had expired.** Sampling was biased toward claims that assert a *protection* or
a *single source* — the two shapes this branch has already shipped wrong.

**True (17).** `hook_dispatch.rs:136–146` (mode narrowing — impl `:875–884`, test
`:1290–1310`); `:127–133` (`MAX_TABLE_BYTES` re-checked *before* the read, `:706`);
`:130` (`MATCHER_MAX_BYTES` re-checked per row, `:744`); `:143`/`:146` (both modes
actually applied — `:416`, `:453`, test `:1127–1128`); `:691–694` ("the runtime
refuses a non-absolute `--table` before it gets here" — `command/hook/argv.rs:159–161`);
`command/hook/list.rs:31–35` ("nothing in this file re-derives" the four gates —
every verdict routes through `status::hook_arming`, `derive_state`,
`merge_not_registered`, `hook_row_state`, `HookArmingInputs`);
`command/hook/run.rs:390–424` (`client_admits` — corrected, and it now states its
own residual about older tables explicitly);
`install/hook_registrar.rs:1027–1075` (`validate_grim_home`'s scope claim and both
named gaps — the `starts_with(workspace)` shape does miss exactly the two shapes it
lists); `:1051–1055` ("`scope` … is asserted below" — test `:1531–1543` iterates both
scopes); `skill/local_pack.rs:43–51` (three guards, each verified independently);
`command/status.rs:655–658` (`hook_arming` is the single derivation of the arming
*gates* — but see Q-B1 for the surface predicate it does not share);
`hook/policy.rs:151–153` (`trust::grants` owns locator normalization);
`hook/audit.rs:595–608` (`capped_line` "bounded by construction" — holds, because
`correlation_id` is grim-minted and short, `command/hook/run.rs:383`);
`oci/hook.rs:175–178` ("the four sites that ask it" — exactly four);
`command/hook.rs:33` + `:96–250` (the ban really does scan every declared runtime
module, and `every_declared_runtime_module_is_checked` closes the widening hole);
`command/hook/run.rs:140–141` ("never panics");
`install/status_badge.rs:150–163` (`iter_artifacts` is the single seam).

**False or expired (8).** `command/status.rs:888–899` ("until it lands" — Q-B1);
`install/path_anchor.rs:986–991` (claims a two-conjunct agreement, implements one —
Q-B1); `install/hook_registrar.rs:203–204` (`HookSync` "the value `sync_config`
logs and `grim status` reads" — Q-W1); `:252–254` ("already logged" — Q-W1);
`:104–114` (`GIT_EXCLUDE_RELATIVE` "for … `grim status`" — Q-S2);
`test/tests/conftest.py:127` ("the hooks dispatch-table schema is not frozen yet" —
T-W1); `test/tests/test_harness_fixtures.py:159–161` ("a `grim` build with no hook
support at all", "WP-O extends this once `grim hook run` exists" — T-W1);
`test/tests/test_hook_run_runtime.py:902–907` ("nothing can install a hook until the
installer's `Hook` branch lands" — T-W4).

**Other enumerations run**: all 17 unit-test `converge_clients` call sites checked
for their scope argument (every one `Project`); all 4 `is_reserved_binding_name`
call sites counted against two competing doc counts; every `HookSync::` reference
outside its own module searched (none); every `converge_clients` caller checked for
use of the return value (none use it); every `ArgvRefusal` variant cross-checked
against an exerciser (`EmptyRoot` has none); `#[cfg(test)]` occurrence counts per
runtime file; `grep` for audit-trail rotation / cap / sanitize coverage across
`test/tests/` (none); `pack_local_artifact` call-site enumeration (11 sites);
`trust_hooks` reader enumeration outside `hook/trust.rs`.

**Deliberately not flagged, per the brief**: anything `cargo fmt` or
`clippy --all-targets -D warnings` enforces; long doc comments that carry reasoning
(the norm on this branch, and appropriate — the ones flagged above are flagged for
being *wrong*, never for being long); pre-existing behaviour the diff does not
touch; abstractions with a single caller; and scenarios already pinned elsewhere —
those are named in T-S3 rather than re-requested.

Status: **COMPLETE.**
