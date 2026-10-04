# Round 2 — correctness

Adversarial correctness review of `git diff 01ce10f..HEAD` (9 commits, `hex/hooks-artifact-kind`).
Scope: defects **introduced by the round-1 fixes themselves**, plus whether the new tests would
still pass if the fix they guard were reverted.

Baseline runs (all green before any finding below was written):

```
cargo test --quiet                                   → 2939 passed; 0 failed
test/ uv run pytest tests/test_hook_run_runtime.py    → 30 passed
test/ uv run pytest tests/test_docs.py tests/test_golden_pre_hooks.py \
      tests/test_bundle_hook_members.py tests/test_hook_decline_dispatch.py \
      tests/test_manual_rig.py                        → 62 passed
```

## Verdict per fix

| Fix | Verdict |
|---|---|
| 1. `matches_tool` alternation (`run.rs`) | **Correct.** No widening. See "Fix 1 — cleared". |
| 2. `expand_payload_dir` (`pipeline.rs`) | **Safe, but the boundary rule is wrong** → F4. |
| 3. `reap_dead_roots` (`hook_dispatch.rs`) | **Two defects** → F1 (Block), F2 (Warn), F3 (Warn, vacuous test). |
| 4. `Vendor::declines_hooks_everywhere` collapse | **Correct.** No call site's semantics changed. See "Fix 4 — cleared". |
| 5. `binding_name_refusal` (`oci/hook.rs`) | **Correct as a control, but not mirrored at the authoring seam** → F5. |

---

## Block

### F1 — the reap silently destroys arming state: invisible in the log AND in the reported outcome

`src/install/hook_dispatch.rs:904-911` (the reap's own report) and `src/install/hook_registrar.rs:732-740`
(what the install reports).

The reap announces itself with `tracing::info!`. grim's default log filter is `warn`:

```rust
// src/main.rs:364
let filter = EnvFilter::try_from_env("GRIM_LOG").unwrap_or_else(|_| EnvFilter::new("warn"));
```

So `dropped N dispatch root(s) whose workspace no longer exists` is **never emitted** unless the
user sets `GRIM_LOG=info`. And the second channel is closed too: a reap-only write returns
`DispatchWrite::Unchanged` by design (`hook_dispatch.rs:929-933`), which
`sync_for_state` maps to `HookSync::Unchanged` / `HookSync::NoHooks`
(`hook_registrar.rs:732-740`). Nothing in `grim install`'s human or JSON output carries a
"roots dropped" field.

Net: **`grim install` deletes another workspace's entire hook arming and reports "unchanged",
with no output at any default verbosity.**

Why this is Block and not Warn: the same hunk of the same function added a *warning* for a
strictly less consequential condition, with an explicit rationale that the diagnostic must be
visible on the install path:

```rust
// src/install/hook_dispatch.rs:806-811
/// `grim hook run` does log that at `warn`, but its stderr is a
/// client's to swallow, so in practice the user sees their hooks stop. The
/// warning therefore has to fire on the *install* path, which is a terminal the
/// user is looking at, and it has to fire before the cliff rather than at it.
```

That argument applies verbatim, and more strongly, to the reap: the size warning describes a
*future* risk, the reap has *already* disarmed a root by the time it logs. A destructive change
to arming state reported at a suppressed level is the silent-guardrail shape C-017 exists to
close (`.claude/rules/arch-threat-model.md`, invariant I6 / `subsystem-cli-api.md` reporting
contract).

Fix direction: `tracing::warn!` at minimum, naming the dropped roots' `entry.root` strings so
the user can tell an intended cleanup from F2's false positive.

---

## Warn

### F2 — `Path::exists()` is not a liveness test, and `DispatchRoot.root` is not a path the reap may trust

`src/install/hook_dispatch.rs:838-846`:

```rust
token == keep || entry.root == "global" || Path::new(&entry.root).exists()
```

Two independent problems.

**(a) The field's own contract forbids this use, and the contract was not updated.**
`DispatchRoot.root` is declared, three commits earlier and still today:

```rust
// src/install/hook_dispatch.rs:586-590
pub struct DispatchRoot {
    /// [`RootScope::display`] — **diagnostics only.** Never matched against
    /// anything at runtime, never compared to the invoking workspace (C-007),
    /// and never derived back into a token.
    pub root: String,
```

The reap now makes this field the sole input to a **destructive** decision, while its doc still
says "diagnostics only". That is not a style point: the producer is deliberately lossy —
`RootScope::display()` is `path.to_string_lossy()` (`hook_dispatch.rs:189`) — precisely because
nothing was supposed to consume it as a path.

Consequence, demonstrated: a workspace whose absolute path contains any non-UTF-8 byte (legal on
Linux) is stored as its U+FFFD-substituted form, and that string does not exist:

```
$ python3 -c "..."          # scratchpad
raw bytes dir exists: True
lossy str: 'ws\ufffd'
lossy path exists: False
```

`Path::new(&entry.root).exists()` is `false` for that live workspace → its root is reaped → its
hooks disarm. (`workspace_key`'s doc at `hook_dispatch.rs:270-277` claims non-UTF-8 workspaces
"never get this far" because `AnchorError::UnknownAnchor` rejects them — but that argument covers
the *workspace-anchored* kinds. A hook payload is anchored at `GrimHome` and its relative path is
all-ASCII (`hooks/payload/<hex>/<name>`), so `strip_prefix_relative`'s `os.to_str()?` at
`path_anchor.rs:1053` never sees the non-UTF-8 component. I traced this but did not execute an
end-to-end install in a non-UTF-8 workspace.)

**(b) `exists()` conflates "gone" with "not visible to this process".** It returns `false` on any
`stat` failure, including `EACCES` on an ancestor, and it is evaluated in the *current* process's
mount namespace. The concrete, non-exotic case: a `GRIM_HOME` shared between a host and a
devcontainer (mounting `~/.grimoire` into the container is an ordinary setup, and nothing in
`AGENTS.md`'s `GRIM_HOME` rules forbids it — only *relative* and *inside-the-workspace* are
refused). Host paths like `/home/u/projects/foo` do not exist inside the container, so **every
`grim install` run inside the container reaps every host root, and vice versa** — the two
environments mutually disarm each other on every install, forever, silently (F1).

`Path::exists()` cannot distinguish that from the deletion the new unit test performs, which is
why the test passes while the behaviour is wrong.

The doc-comment at `hook_dispatch.rs:826-830` addresses the *unmounted-share* variant ("reaped
and re-armed by the next `grim install` in it"), but that reasoning does not hold for a shared
`GRIM_HOME`: the workspace is fully live and in use, and each side's re-arm is immediately undone
by the other side's next install.

Fix directions, cheapest first: require the absence to be **positively confirmed**
(`symlink_metadata` returning `NotFound`, not any `Err`) rather than trusting `exists()`; and/or
gate the reap on an explicit opt-in / `grim install --prune-dispatch` rather than making it a
side effect of every install.

### F3 — `the_converging_root_is_never_reaped_by_its_own_write` is vacuous: it passes with the exemption removed

`src/install/hook_dispatch.rs:1391-1412`.

The test's single `converge_root` call is the **first** write to a fresh `tempdir`. Trace:

1. `dispatch.json` does not exist → `read_table` (`hook_dispatch.rs:898`) yields the **empty** table.
2. `reap_dead_roots(&mut table, token)` (`:904`) calls `table.roots.retain(...)` over **zero**
   entries — the `token == keep` guard on `:842` is never evaluated, and `reaped == 0`.
3. `desired` is inserted → `DispatchWrite::Written`, and the token is present.

Deleting `token == keep ||` from `:842` therefore cannot change this test's outcome. The assertion
message ("the root being converged reaped itself; a first install would arm nothing") describes a
failure the test's own setup makes unreachable.

To actually pin the exemption, the root must already be in the table when the reap runs — i.e.
converge once with an existing workspace, remove the workspace, then converge **that same root**
again and assert `DispatchWrite::Unchanged` (without the exemption you get `Written`, because the
reap drops the row and the desired entry re-inserts it).

Related, worth noting while fixing: the exemption is *only* observable in the reported
`DispatchWrite` and in whether the extra reap-only write happens. In every case the row ends up
correct either way. If that is the intended contract, the test should assert the outcome variant,
not the row's presence.

### F4 — `expand_payload_dir`'s braceless boundary rule makes the two documented forms disagree

`src/command/hook/pipeline.rs:962-980`. The doc at `:957-960` states the invariant:

> `$GRIM_HOOK_DIRECTORY` is left intact rather than mangled into `<dir>ECTORY`: a shell would read
> that as a different variable, and **the two forms must not disagree about the same string**.

The implemented rule is "expand only when followed by `/` or end-of-element". A shell's rule is
"expand unless followed by an identifier character `[A-Za-z0-9_]`". Those differ for every other
delimiter, and the stated invariant is violated. Reproduced with the function's exact body
(`scratchpad/exp.rs`, `payload_dir = /P`):

```
--path=$GRIM_HOOK_DIR:$GRIM_HOOK_DIR/lib   -> --path=$GRIM_HOOK_DIR:/P/lib
${GRIM_HOOK_DIR}:${GRIM_HOOK_DIR}/lib      -> /P:/P/lib               <-- disagrees with the line above
$GRIM_HOOK_DIR$GRIM_HOOK_DIR/x             -> $GRIM_HOOK_DIR/P/x      <-- a shell gives /P/P/x
$GRIM_HOOK_DIR.bak                         -> $GRIM_HOOK_DIR.bak      <-- a shell gives /P.bak
"$GRIM_HOOK_DIR"                           -> "$GRIM_HOOK_DIR"
```

Impact is functional, not security (the value substituted is always grim-derived; a failure to
substitute is fail-closed). But because `argv` is exec form, **nothing downstream will ever expand
the surviving literal** — the handler receives `--path=$GRIM_HOOK_DIR:/P/lib` verbatim. A
publisher writing a `PATH`/`PYTHONPATH`-shaped argument gets a half-expanded string and a silent
misbehaviour, which is the same class of defect (documented form does not work) that this fix
exists to close. The new unit test at `pipeline.rs:1055-1100` covers only the `/`-and-end cases,
so it does not catch this.

Fix: replace `tail.is_empty() || tail.starts_with('/')` with a rejection of an identifier
continuation — expand unless `tail` starts with `[A-Za-z0-9_]`. That keeps
`$GRIM_HOOK_DIRECTORY` intact (the case the current rule was written for) and makes every row
above agree with both the braced form and a shell.

### F5 — a hook name the install seam refuses still builds and releases, and the new docs row claims otherwise

`src/oci/hook.rs:219-233` (the refusal), `src/oci/hook.rs:650-665` (what `grim build` actually
validates), `catalog/skills/grim-authoring/references/hook-spec.md:390` (the new claim).

`binding_name_refusal` is reachable at `installer.rs:456-460` and `hook_registrar.rs:1213`, and
`add.rs:239-250` / `:520-529` run the same two checks separately. It is **not** reachable from
`grim build`: `HookManifest::validate` runs `validate_reserved_name()` and the name-equals-stem
rule, and never `SkillName::parse`.

Demonstrated with the release binary — a hook whose name is a valid directory stem but not a plain
artifact name builds clean:

```
$ grim build $S/hb/my_hook --kind hook
Kind  Name     Path                     Layer Digest        Status
hook  my_hook  …/scratchpad/hb/my_hook  sha256:3409be33…    built
$ echo $?
0
```

That artifact is then **unusable by every consumer**: `grim add` refuses the binding name
(`add.rs:239`, `CommandError::InvalidBindingName`) and `install_one` refuses it
(`installer.rs:456`, warn + `InstallOutcome::Skipped`). A publisher can build, `grim release`, and
push a hook that nobody can install, and learns nothing at authoring time.

The new docs row makes this worse by asserting the opposite. `hook-spec.md:384` introduces its
table with "Every row below fails `grim build` with **exit 65**", and `:390` adds:

```
| Artifact name is not a plain name (`../x`, `a/b`) | `is not a usable artifact name` |
```

Both halves are wrong: `grim build` returns 0 (above), and the quoted message string exists only
in `binding_name_refusal` (`src/oci/hook.rs:222` — grepped, it appears nowhere else in `src/`).
The row's own examples are also unreachable at build time for a second reason: `../x` and `a/b`
cannot be a directory stem, and a `name` that differs from the stem fails rule 7 first.

Fix: add `SkillName::parse` to `HookManifest::validate` (alongside the existing
`validate_reserved_name()` — it is the same "is this a name at all" question, one seam earlier),
which makes the docs row true and closes the publish-an-uninstallable-artifact gap in one change.

### F6 — `converge_root` still writes a table `read_table` will refuse; the new warning only fires at 80 %

`src/install/hook_dispatch.rs:946-957` vs `:706`.

`read_table` degrades to the **empty** table once the file exceeds `MAX_TABLE_BYTES`
(`:706`, `meta.len() > MAX_TABLE_BYTES`), which disarms every hook for every root. The fix adds a
`warn!` above 80 % of the cap but no refusal, so `atomic_write` at `:958` will happily produce a
file that this same binary cannot read back.

The codebase already has the opposite convention for exactly this situation, with the rationale
spelled out:

```rust
// src/lock/lock_io.rs:44-50
/// The serialized lock is measured against the same
/// [`config::FILE_SIZE_LIMIT_BYTES`] the load path enforces: writing a
/// lock this build would refuse to read back is never correct — every
/// later command, including the one that would undo the growth, fails on
/// a file grim itself produced. The check runs before the write, so the
/// previous (readable) lock survives.
```

That argument transfers exactly, and the consequence here is worse than a failed command: the
previous table *was* readable and armed, and the oversize write disarms the whole machine. The
warning also does not change register once the cap is passed — at 1.2 MiB it still reports
"X of its 1048576-byte limit", never "you have exceeded it and nothing is armed".

Fix: refuse the write past `MAX_TABLE_BYTES` (leaving the previous readable table in place) and
keep the 80 % warning as the early signal it was added to be.

---

## Suggest

### S1 — `grim hook list` derives a payload path from a record name with no containment guard

`src/command/hook/list.rs:271`:

```rust
let path = hook_dispatch::payload_dir(grim_home, root, name).join(HOOK_MANIFEST_FILE);
```

`hook_registrar::desired_entries` resolves the same path through
`AnchoredPath { anchor: GrimHome, … }.resolve(roots, Containment::Strict)`
(`hook_registrar.rs:1230-1236`) **and** now `binding_name_refusal` (`:1213`). `list.rs` does
neither. With fix 5 in place no new record can carry a traversing name, so this is only reachable
via a hand-edited or pre-fix state file, and the effect is a read (an arbitrary file parsed as
`hook.toml` and surfaced in `grim hook list`), not a write. Still: the fix's own doc calls
`desired_entries` the belt-and-suspenders half for exactly such records, and this is the third
consumer of `payload_dir(…, record.name)` that was left out.

### S2 — `add.rs` is now the fourth spelling of the binding-name question

`src/command/add.rs:239-250` and `:520-529` run `SkillName::parse` then
`is_reserved_binding_name` as two separate checks, i.e. `binding_name_refusal`'s body inlined.
Justified in part — `add` needs the typed `CommandError::InvalidBindingName` /
`ReservedBindingName` variants, which a `String` reason cannot produce — but it sits awkwardly
beside `7f54c2b`'s stated purpose (collapse duplicated predicates) and beside
`binding_name_refusal`'s own doc ("Two questions, one answer, and the order matters"). If the two
diverge later, `add` and `install` will disagree about the same name. Consider having
`binding_name_refusal` return a typed reason both sites can map.

### S3 — the glob branch of `matches_one_alternative` is unreachable for any armed entry

`src/command/hook/run.rs:481-483`. Confirmed by trace, not a defect — recorded because the new
test row at `run.rs:568-573` (`Ba*|Read` → `Bash` → `true`) asserts behaviour for an input that
can never reach a dispatch row, and a future reader may take it as evidence that glob matchers
arm. `classify_matcher` (`vendor.rs:395-400`) returns `NotTranslatable` for anything that is not
whole-string `*` or an alternation of exact names, and `hook_registration`
(`vendor.rs:1160-1166`) turns that into `HookDecline::MatcherNotLossless`; only accepted
registrations contribute dispatch rows (`hook_registrar.rs:592-600, 624-645`).

---

## Fix 1 — cleared (no widening)

Attacked every axis named in the brief:

- **matcher is only `|`** → `split('|')` yields `["", ""]`; both take the exact-name path
  (`run.rs:481`) and no tool is named `""` → matches nothing. Fail-closed.
- **empty alternative** (`Bash|`, `|B`, `A||B`) → same, matches nothing in grim. It matches
  *everything* on claude/codex, so grim narrows — but the case is unreachable anyway: both
  `grim build` and the **install-side** `classify_matcher` split identically and require
  `is_exact_tool_name` for each alternative (`vendor.rs:398`), so a hand-crafted OCI artifact
  carrying `"Bash|"` declines at install rather than arming.
- **glob inside an alternative** → unreachable, see S3.
- **agreement with each vendor's own field** → for an armed `ExactOrAlternation` matcher the
  vendor field carries the string verbatim (`vendor.rs:1163`); claude/codex are start-anchored
  tail-open regexes, so they fire on a *superset* (`Bash` also fires on `BashOutput`) and grim's
  exact pass narrows it. Narrowing is the intended direction for the authoritative pass (C-006).
- **`MATCHER_ALLOWED`** (`oci/hook.rs:72`) admits `|`, so the previous whole-string compare really
  did arm-everywhere/fire-nowhere. The fix is the right shape.

The acceptance test `test_an_alternation_matcher_fires_on_every_alternative`
(`test/tests/test_hook_run_runtime.py:335`) is mutation-resistant: reverting the split makes both
positive legs fail, and the negative leg is not vacuous — `HOSTILE_PAYLOAD` contains the exact
literal `"tool_name" : "Bash"` (`:57`) that `_run_for` substitutes, so a failed substitution would
fail the `Write` leg rather than pass it.

## Fix 4 — cleared (no semantic change at any call site)

`Vendor::declines_hooks_everywhere` (`vendor.rs:871-874`) is `self.hook_surface().is_none()`, has
no override anywhere (grepped: only the default plus five call sites), and every collapsed site
was previously spelled `hook_surface().is_none()` / its negation:

| Site | Before | After | Same? |
|---|---|---|---|
| `client_target.rs:762` | `hook_surface().is_none()` | `declines_hooks_everywhere()` | yes |
| `installer.rs:1234` | `hook_surface().is_none()` | idem | yes |
| `path_anchor.rs:994` | `hook_surface().is_none()` | idem | yes |
| `hook_registrar.rs:494` | `hook_surface().is_some() && kind_surface(…)` | `!declines… && kind_surface(…)` | yes |
| `installer.rs:1204` | `hook_surface().is_some() && kind_surface(…)` | idem | yes |

The scope-aware and scope-blind variants were **not** conflated: `kind_is_permanently_declined`
(`installer.rs:1231-1236`) and `is_declined_global_pair` (`path_anchor.rs:991-996`) still take
only the capability half, and `client_supports_kind` / `hook_clients` still `&&` in
`kind_surface(kind, scope)`.

`status.rs::client_has_hook_surface` (`:904-906`) now delegates to
`client_supports_kind(client, Hook, &self.workspace, self.scope)`. The `Hook` arm
(`installer.rs:1204`) **ignores `workspace` entirely**, so the new `HookArmingInputs.workspace`
field (`status.rs:822-826`) changes nothing `status` reports for any client — it is a
correctly-plumbed unused parameter, exactly as its own doc says.

## Fix 5 — cleared as a control (see F5 for the seam it misses)

- **No false positives on legitimate names.** `binding_name_refusal` refuses exactly what
  `grim add` already refused for hooks (`add.rs:239`, `SkillName::parse` — added in the same
  round), so nothing a user could previously declare is newly rejected. `SkillName`'s grammar
  (`skill/skill_name.rs:47-81`) admits `x.y`, `a1.b2-c3`, single-char names; the test's negative
  control at `oci/hook.rs:1635-1643` covers them.
- **The reserved names still refuse for the reserved reason.** All three of
  `RESERVED_ARTIFACT_NAMES` are valid `SkillName`s (`bin`, `payload`, `dispatch.json` — the last
  parses as `dispatch` `.` `json`), so the parse check does not shadow them; the test at
  `:1622-1631` pins that.
- **Reachability on the write path.** `install_one`'s gate (`installer.rs:453-461`) precedes
  materialization. The uninstall/reap side is independently safe: a traversing name cannot be
  anchored, because `strip_prefix_relative` (`path_anchor.rs:1050-1057`) rejects any non-`Normal`
  remainder component, so it degrades to `AnchorError::UnknownAnchor` rather than deleting outside
  `$GRIM_HOME`. `hook_registrar.rs:1213` adds a third guard on the arming path (which already had
  `Containment::Strict`). The one uncovered consumer is `hook/list.rs` → S1.

## New tests — mutation assessment

| Test | Would it fail if the fix were reverted? |
|---|---|
| `run.rs` `matches_tool` table rows for `Bash\|Read` | Yes — the three alternation rows flip. |
| `test_an_alternation_matcher_fires_on_every_alternative` | Yes (verified reasoning above). |
| `pipeline.rs::the_payload_dir_token_expands_in_every_argv_element` | Yes (function would not exist); covers only the `/`/end boundary → F4. |
| `pipeline.rs::the_payload_dir_token_expands_in_argv_zero_too` | Yes. Note the shape it asserts is refused by `grim build` (`payload_relative_file`, `oci/hook.rs:860-879`), so it pins the uniform rule, not a reachable manifest. |
| `pipeline.rs::the_shell_form_is_not_pre_expanded` | Signature-coupled only; guards a *future* regression, which is its stated purpose. Acceptable. |
| `test_the_documented_payload_dir_token_runs_the_payload` | Yes — literal token, `sh` fails, marker absent. |
| `hook_dispatch.rs::a_dispatch_root_is_reaped_only_once_its_workspace_is_gone` | Yes — `roots.len() == 2` fails without the reap. Good positive control. |
| `hook_dispatch.rs::the_converging_root_is_never_reaped_by_its_own_write` | **No** → F3. |
| `oci/hook.rs::a_binding_name_that_is_not_a_plain_name_is_refused` | Yes, and it carries both a reserved-reason check and a negative control. Strong. |
| `trust.rs` (7 new tests) | Coverage for previously untested code, not fix-guards. Each isolates one field of an otherwise-granting entry and asserts the *property* through `decide`/`grants`, not the spelling — e.g. `a_bare_host_entry_gains_a_sibling_never_a_flag` fails if `persist_grant` switches from equality to the prefix rule. No vacuity found. |
| `audit.rs` (3 new tests) | Strong. The rotation test asserts the exact `size < MAX_LOG_BYTES` off-by-one on both sides and that the single retained generation is *replaced* (`first() == b'b'`); the elision test asserts which field survives each rung, not merely that the result fits. |
| `test_golden_pre_hooks.py` | Non-vacuous despite seeding the golden locks: `lock_io::save` (`src/lock/lock_io.rs:57-79`) has no unchanged-early-return and always `atomic_write`s, so the comparison is fresh serialization vs. golden bytes. The state files are unseeded, and the hash test runs unseeded. |
| `test_docs.py::test_every_hook_arming_cause_is_documented` | Parses 9 tokens against 9 `HookArmingCause` variants (verified by script); the regex picks up future arms automatically, so `>= 9` is a parse sanity floor rather than a ceiling. Sound. |
| `test_hook_decline_dispatch.py` positive control | Real — `watch_marker` is written by the sibling that does register, so the negative leg can no longer pass on a dispatcher that spawns nothing. |

---

## Not defects (checked and dismissed)

- **Fix 2 as a security escape.** The substituted value is always grim-derived
  (`envelope.rs:441` supplies the identical string to the env for the `command` form), argv is
  exec form so no quoting is needed and no re-parse happens, and reaching outside the payload dir
  via `${GRIM_HOOK_DIR}/../…` grants nothing a plain absolute `argv[0]` did not already grant to a
  publisher whose registry the user has trusted for hooks. The `command` form is provably not
  double-expanded (`pipeline.rs:899-902` + the test at `:1114`).
- **Fix 2 loop termination / slicing.** `BARE.len() == 14`, all-ASCII, and the non-matching branch
  advances `rest` by at least 14 bytes, so no panic and no infinite loop.
- **The reap making the table grow.** `retain` only removes; `entry.root == "global"` exempts the
  global root, whose `root` field is the literal `"global"` (`RootScope::display`,
  `hook_dispatch.rs:186-191`) and is never a path.
- **Concurrent installs racing the reap.** `converge_root` holds the advisory lock across
  read → reap → write (`hook_dispatch.rs:883-958`); a competing install gets
  `DispatchError::Locked` → cause `dispatch-lock-held`.
- **A relative `root` string reaching the reap.** `root_scope_for` (`hook_registrar.rs:1346`)
  takes `ResolvedScope::workspace` (`scope_resolution.rs:42`), which comes from project
  discovery. I found no path that stores a relative workspace, so the CWD-dependence of
  `Path::new(rel).exists()` is not reachable — noted only because F2(b) is the same class of
  problem reached a different way.
- **`DispatchWrite` outcome mapping after a reap.** All four combinations of
  (reaped, this-root-equal, desired-present) map correctly at `hook_dispatch.rs:920-942`; the
  reap-only `Unchanged` is honest about *this root* (its aggravating effect on reporting is F1,
  not a mapping bug).
