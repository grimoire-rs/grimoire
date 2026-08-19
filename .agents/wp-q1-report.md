# WP-Q1 — remediation of hooks audit findings P-1, P-2, P-3 and P-6

**Worktree** `/mnt/wsl/share/dev/grimoire/grimoire/.agents/worktrees/wp8-q1`
**Branch** `hex/hooks-artifact-kind--wp8-q1`, base `f827ecb`

| Commit | Finding |
|---|---|
| `05b6d20` `fix(install):` | **P-1** (Block) — a `HookDecline` now keeps the hook out of the dispatch table, and reads as not armed |
| `6947364` `fix(oci):` | **P-3** (Warn) — `hook.toml` is re-validated at install |
| `61f792e` `fix(install):` | **P-2** (Warn) — a reserved *binding* name is refused before it materializes |
| `a9a115f` `fix(oci):` | **P-6** (Suggest) — `id` is charset-bound, and no longer interpolated into a path |

All four findings landed. `P-4` was the lead's and is untouched
(`projector.rs` not modified).

---

## P-1 — the shape chosen, and why

**Shape (1), the inverted ordering.** `hook_registrar::converge_clients` now resolves
the launcher path, the table path and the root token **before** the per-client desired
sets, and a new `register_desired` (`src/install/hook_registrar.rs`) runs
`Vendor::hook_registration` **once** per `(client, hook)` and feeds both consumers from
its accepted set: `union_of` (the dispatch table) and `sync_for_state` (the client's own
surface). `sync_for_state` no longer calls `hook_registration` at all — it is handed the
result.

The invariant, stated in the module doc: **a row exists for `(client, hook)` if and only
if that client's registration was written.** That is what makes `run.rs`'s decline-free
selection key `(root, client, event)` sufficient.

Shape (2) — a per-row `registered` flag — was rejected for the reason the brief gave: it
puts a declined row in the table and asks every reader to filter it. Since the table
format has never shipped, absence costs nothing that a flag would have bought.

**The one side effect that moved.** `hook_registration` takes the launcher, the table and
the root token, and `root_token` mints the machine key on first use. Because the verdict
now gates the union, that mint happens ahead of the launcher write rather than after it.
Both were already unconditional past `arming_refusal` — a pure reap derives the token too,
because it has to name the root it is emptying — so no path gained a write it did not
have, and `arming_refusal` still precedes every one of them ("a refusal writes nothing at
all" is unchanged, and `a_refusal_writes_nothing_and_reports_every_client` still passes).
Recorded in `converge_clients`' doc under "The step order inverted for P-1, and what that
cost".

**The rejected third option, recorded because it is the tempting one.** Computing the
verdict twice — once with probe paths to build the filter, once with the real paths for
the surface — would work today, because the refusal order provably does not read those
three values (`client_target.rs`'s `HOOK_CELL_PROBE_LAUNCHER` pins exactly that). It was
rejected because a contract that depends on two calls agreeing is the shape P-1 already
was. Nothing re-derives the refusal order; there is still exactly one spelling of it.

## Executed before / after — the declined payload no longer runs

**Before** (`f827ecb`, release build of the unmodified tree). The audit's own
demonstration test passes, i.e. the defect reproduces:

```
$ cd test && GRIM_COMMAND=$PWD/bin/grim uv run pytest tests/test_hook_decline_dispatch.py -q
2 passed in 0.28s
```

That test asserted, against the real binary and a real registry push, that the declined
mutator's row was present (`hooks.len() == 2`), that `grim hook run --client claude --event
PreToolUse` spawned it (`marker.is_file()`), and that its rewrite reached claude's
`hookSpecificOutput.updatedInput`.

**After**, same command on the final tree — the assertions inverted:

```
$ cd test && GRIM_COMMAND=$PWD/bin/grim uv run pytest tests/test_hook_decline_dispatch.py -q
4 passed in 0.28s
```

The dispatch table written by that install, verbatim (probe output, two-entry artifact,
`watch` observer + `rewrite` mutator, both `PreToolUse` on `Bash`):

```json
"hooks": [ { "artifact": "shell-guard", "id": "watch", "client": "claude",
             "event": "PreToolUse", "tier": "observer", "matcher": "Bash", ... } ]
```

One row. `rewrite` has none, the marker file is not written, and no `updatedInput`
reaches claude.

**Every new test fails against the current code.** Verified by execution, not asserted:

| Test | How it was shown to fail before |
|---|---|
| `test_a_declined_mutator_is_not_dispatched_p1` | `git stash push -- src/`, rebuild, run → `assert 'installed' == 'not-armed'` |
| `test_an_artifact_whose_every_entry_is_declined_reads_not_armed_on_both_surfaces_p1` | same run → FAILED |
| `test_a_manifest_grim_build_would_reject_does_not_reach_the_dispatch_table_p3` | same run → `assert 'grim build' in <the pre-fix stderr>` |
| `hook_registrar::tests::a_declined_mutator_is_kept_out_of_the_dispatch_table_p1` | the fix cannot be stashed (the test lives in `src/`), so `register_desired` was temporarily edited to push the row on a decline → FAILED |
| `hook_registrar::tests::a_manifest_the_build_rules_reject_arms_nothing_p3` | same temporary edit (`validate_installed` call disabled) → FAILED |

Two of the new tests **do not** fail before, and saying so is part of the report:
`hook_registrar::tests::the_install_time_recheck_does_not_apply_the_name_equals_stem_rule`
pins the *absence* of a rule (a renamed binding must keep arming), and the
`merge_not_registered` / `validate_installed` unit tests exercise functions that did not
exist. They are regression pins for the new behaviour, not reproductions of the defect.

The P-2 demonstration test in the same file is untouched and still asserts its defect.

## What `hook list` and `status` now say about a declined entry

Removing the row was **not** most of it. `status::hook_arming` consults the table first,
but `armed_pairs` was keyed `(client, artifact)`: with one entry registered and one
declined, the pair is still present, so the declined entry still read `arming: []` — and
with *no* entry registered the pair is absent, the config chain runs, every gate passes,
and the fallthrough is "armed and running: no verdict", so it read `arming: []` again.
Verified by execution before changing anything.

So the reporting half needed a cause that did not exist. Added:
`HookArmingCause::NotRegistered` → token `not-registered`, `state()` → `not-armed`,
`transient()` → `false`. `HookArmingInputs::armed` became `(client, artifact, entry-id)`
**triples**, with `arms_artifact` / `arms_entry` accessors, and one shared merge
(`status::merge_not_registered`) applied by both surfaces.

`grim hook list` (executed, two-entry artifact, `rewrite` declined by decision K):

```
Hook                 Tier     Events      Client  State      Detail
shell-guard/rewrite  mutator  PreToolUse  claude  not-armed  grim registered nothing here for this client, so nothing runs — usually its tier at that event or its matcher; re-run grim install to see the reason it reports
shell-guard/watch    observer PreToolUse  —       installed  —
```

```json
{ "artifact": "shell-guard", "id": "rewrite", "tier": "mutator", "state": "not-armed",
  "arming": [ { "client": "claude", "cause": "not-registered",
                "message": "grim registered nothing here …", "transient": false } ] }
```

`grim status` for the **same** artifact reads `state: "installed"`, `arming: []` — and
that is correct at its granularity: one entry *is* registered, and a `status` row has no
entry dimension to carry the difference. Asserted explicitly in the test so it is a
recorded decision rather than an oversight.

For an artifact whose **every** entry is declined (a match-all `mutator` at `PreToolUse` —
accepted by `grim build`, refused by decision K), with trust granted durably
(`trust_hooks = true` in the global config), **both** surfaces report it (executed):

```
hook list : state "not-armed", arming [{claude, not-registered}]
status    : state "not-armed", arming [{claude, not-registered}]
```

The merge rules, each pinned by a unit test in `status.rs`:

1. the client must have a recorded `ClientOutput` — otherwise a never-installed hook
   would read "not registered" instead of letting its `missing`/`stale` lifecycle token
   tell the real story;
2. an existing verdict wins — a gated/untrusted/surface-less/`GRIM_HOME`-refused client
   already carries the actionable cause, and that cause *also* explains the missing row;
3. the exception to (2) is `ClientTrustPending`, which asserts a written registration the
   client has not approved. With no row there is nothing to approve, so it is replaced.

## P-3 — what the install-time revalidation checks, and what it does not

`HookManifest::validate` is split into `validate_reserved_name`, the `name`-equals-stem
check, and a shared `validate_entries`. `desired_entries` now calls the new
`HookManifest::validate_installed` — reserved name + every per-entry rule (1 handler
ambiguity, 2 tier/event, 3 matcher charset and length, 4 C-019's payload-relative first
token, 5 duplicate `id`s, 6 reserved client keys, 9 names-a-moment) — against the
**materialized payload directory**, which is the installed artifact's own directory, so
rule 4's filesystem probe asks the same question it asks at build.

A failure drops that **one artifact** with a warning and the command still exits 0 (I3).
The whole-artifact drop is deliberate: the rules are cross-entry (duplicate `id`s), and a
manifest grim would not have built is not one to arm half of. It is a gentler degrade than
the pre-existing unparsable-manifest path, which fails the projection for the whole
client; both are named at the site.

**Deliberately not re-checked, each named in `validate_installed`'s doc:**

- **Rule 7, `name` equals the directory stem.** At install the stem is the user's
  *binding* name (`[hooks] my-guard = "…/shell-guard:1"`), which is theirs to choose.
  Applying the build rule here would refuse every renamed binding — pinned by
  `the_install_time_recheck_does_not_apply_the_name_equals_stem_rule`, because the
  omission is invisible in the happy path.
- **The binding name against `RESERVED_ARTIFACT_NAMES`.** Rule 8 is re-checked against
  `self.name`, the manifest's own name, only. A *binding* called `bin` or `payload` still
  materializes over the launcher namespace — that is the audit's separate finding P-2,
  and it belongs at the seam that chooses the payload directory.
- **`HookEntry::id`'s charset** (P-6). Unvalidated at build too; adding it on one side
  only would make the two seams disagree about what publishes.
- **Anything a client decides** — whether a tier is honourable at an event *for a given
  client*, and whether a matcher translates losslessly. That is
  `Vendor::hook_registration`'s per-client verdict, enforced by that call.

I5 compliance: nothing here is described as prevention that is not. The build-time rules
are now a boundary for the rules listed above and remain ergonomics for rule 7 and the
binding name, and the doc says which is which.

## P-2 — refused before materialization, and in two more places

**Where the refusal had to go, and why the brief's site was not enough.** The brief
said to land it "in the same place you land P-3's revalidation" — `desired_entries`,
the arming seam. That is too late for the finding's own sharp edge, and the audit
says so itself ("before materialization, so nothing is half-written"):
`desired_entries` runs *after* the payload is written, and the payload tree is
exactly what `grim uninstall` reaps. A refusal there would withhold the arming and
still leave `$GRIM_HOME/hooks/bin/` on disk for the reap to take the launcher with.

So the control is `installer::install_one`, beside the S-001 policy gate — before
the integrity gate, before the fetch, before any write. Two further places ask the
same question through the same predicate (`oci::hook::is_reserved_binding_name`),
and neither could be the control:

- `hook_registrar::desired_entries` — for a record that predates the gate or was
  hand-written; grim must not read an armed manifest out of its own launcher
  directory. (This is the brief's site, kept.)
- `grim add`, both the registry and the path-source path — the friendlier error at
  the moment a user types the name, exit 64, new
  `CommandError::ReservedBindingName`. **Chosen deliberately as "both, not only
  `add`"**: a bundle picks its members' binding names and they never pass through
  that command, exactly as the brief warned.

No record is written on the refusal, unlike the S-001 skip beside it. A zero-output
record exists so `uninstall` can still reach a materialized payload; there is
nothing here to reach, so `grim status` reports the declared row as `missing`
beside the install-time warning, which is what it is.

**Executed evidence.** The audit's demonstration test passed at `f827ecb` (the
payload materialized `$GRIM_HOME/hooks/bin/{hook.toml,grim-hook}` and the uninstall
deleted the launcher). Inverted and **strengthened**: the test now declares a
second, validly-bound hook so the launcher genuinely exists and is armed, then
asserts `grim uninstall --global hook bin` leaves it intact — "the reap cannot take
the launcher" is only assertable against a launcher that is there to take. Against
the pre-fix binary the same test fails at the first new assertion, with the install
stdout showing the reserved binding materializing:

```
FAILED test_a_reserved_binding_name_is_refused_before_it_materializes_p2
assert 'reserved' in ''      # pre-fix: no warning, and hooks/bin/ was written
```

**Both documents corrected**, as instructed: `src/command/add.rs`'s comment
("enforced at the install seam rather than here") now describes the real three-way
split, and `.claude/rules/subsystem-file-structure.md` § Hooks no longer claims a
`payload` artifact "cannot materialize" — it says which check holds which string,
and records what the reap did before the fix.

**Residual, named rather than engineered against.** A record written by a grim that
predates the install gate still names `$GRIM_HOME/hooks/bin` as its output tree,
and `uninstall` would still reap it. Unreachable in practice — the hook kind has
never shipped, so no such record exists outside a hand-edited state file (N1/N2) —
and closing it means teaching the reaper to refuse grim's own paths in `prune.rs`,
which nothing else needs.

## P-6 — the path sink closed by removing the interpolation, not by guarding it

**The sink I fixed.** `write_payload_file` now names its file
`payload-<pid>-<slot>.json` — two integers grim owns — instead of interpolating
`entry.artifact` and `entry.id`. `slot` is a process-local `AtomicU64` rather than
an index threaded from the caller's loop, so it stays correct if the tier pipeline
ever stops being serial. The readable `artifact/id` moved to a `debug` line, since
the name no longer says whose file it is.

**I did not use the hash the finding asked for, and the reason is a shipped
contract.** C-009 forbids the runtime from hashing anything, and
`hook::tests::the_runtime_computes_no_digest_c009` enforces it as a *source-level*
ban on `Sha256`, `.hash(`, `Algorithm::` and `crate::store::hash` in every runtime
file. That guard exists so nobody re-adds the exec-time integrity re-check decision
A3 deleted (defending against N2, a non-goal, on the hot path of every tool call).
I wrote the `SHA-256(artifact ‖ 0x00 ‖ id)` version first; the test caught it:

```
test command::hook::tests::the_runtime_computes_no_digest_c009 ... FAILED
```

Weakening that guard to admit a "name-derivation" hash would trade a real,
enforced invariant for a Suggest-tier convenience, so the name derivation dropped
the digest instead. Two integers need no primitive at all, and the property the
finding wanted — *no caller-supplied byte reaches the path* — is stronger this way
than with a hash.

**The authoring half, at both seams.** `hook_id_char_allowed` (ASCII alphanumeric
plus `_`, `-`, `.`) and `HOOK_ID_MAX_BYTES` (128) are rule 10 of
`HookManifest::validate`'s per-entry pass, which P-3's `validate_installed` shares
— so the rule holds at `grim build` *and* against the materialized manifest at
install, which is what the brief asked for and what keeps the two seams agreeing
about what publishes. Narrower than `matcher_char_allowed` deliberately: an `id`
names one entry inside one artifact, so `*`, `?`, `/` and `|` buy nothing, and `/`
is what made the traversal probe look plausible. Length before charset, so a
rejected `id` quoted into a diagnostic is already bounded.

With the path sink closed this rule is **defence in depth, not the control**, and
it says so where it is declared (I5).

**The sink I left, and why.** `tracing` is not sanitized. The envelope drops any
value carrying a brace, bracket or control character (`envelope::is_flat_scalar`)
and `AuditRecord::new` sanitizes on the way in, so those two are closed; a log line
is not. What holds it is upstream: no row grim arms can carry an escape sequence,
because `id` is now validated at both seams. The gap that survives is a **dispatch
table written by a grim predating rule 10** — `read_table` re-checks the matcher
length and `payload_dir`, never the `id`. I deliberately did not add it there:
that reader rejects the **whole table** on a bad row, so one stale `id` would
disarm every hook on the machine, which is the wrong trade for a cosmetic sink.
The next `grim install` rewrites the root wholesale. Named in a comment at the
site where `hook` is built in `pipeline.rs`.

**Executed evidence.** New acceptance test
`test_a_traversal_shaped_hook_id_never_reaches_the_dispatch_table_p6` hand-pushes
the audit's exact `id` shape, asserts the install still exits 0, that no row
reaches the table, and that the escape directory is not created. Against the
pre-fix binary it fails at the first new assertion (the install armed the hook
with no warning).

## Errors and soft spots found in the audit itself

1. **§ *Errors found in prior artefacts* item 4 is resting on the defect.** It records
   that WP-R's F-2 "no longer reproduces at `1d60462`" because `grim status` reports
   `state: "installed"`, `arming: []` after an `--allow-hooks` install. That answer came
   from the *declined row being in the table* — the table is consulted first, and the row
   was there whether or not anything registered. With P-1 fixed, an artifact armed through
   `--allow-hooks` whose rows are **all** declined falls back to the config-derived
   verdict and reports `gated` / `registry-not-trusted` again (executed). F-2 itself stays
   closed — a hook that genuinely *is* armed still reports armed — but the observation was
   evidence of the bug, not of a control. Recorded in the audit's new remediation log.

2. **§ P-1's remediation direction mislocates the fix.** It reads "filter `union_of`'s
   input by the same `hook_registration` verdict `sync_for_state` computes (one call, two
   consumers)". The verdict *was* computed in `sync_for_state`, but the fix could not stay
   there: `hook_registration` takes the launcher, the table and the token, all of which
   step 4 derived **after** the desired sets. The ordering had to invert, and that carries
   a real (small) consequence the direction did not anticipate — the machine-key mint moves
   ahead of the launcher write. The brief spotted this; the audit did not.

3. **§ P-1's "reporting half" understates the work.** "Either way `grim hook list` must
   report a declined entry as not armed for that client" reads as a consequence of removing
   the row. It is not: `armed_pairs` is artifact-keyed and the config chain's fallthrough is
   "no verdict", so removing the row leaves *both* surfaces reporting `arming: []`. A new
   cause was unavoidable.

4. **P-3's "four downstream controls hold" is accurate, and one of them is thinner than it
   reads.** "grim's own matcher treats a metacharacter it did not sanction as a literal" is
   true of `run.rs::matches_tool`, but the *vendor* matcher is what gets grim invoked at
   all, and `classify_matcher` sends `Bash$(id)` down `MatcherNotLossless` — so the hook is
   declined rather than mis-matched. The net effect is the same and the audit's conclusion
   stands; the two controls are simply not independent in the way the list implies.

5. **P-2's own remediation direction understates where the check has to go.** It
   offers a choice — "reject `RESERVED_ARTIFACT_NAMES` for `record.name` at the
   install seam ..., **or** correct both documents". The parenthetical "(before
   materialization, so nothing is half-written)" is doing the real work and is easy
   to miss; the arming seam is an install seam too, and a fix landed there would
   have left the reap intact, which is the finding's own sharp edge. Worth
   promoting from a parenthetical to the requirement.

6. **P-6's remediation direction is unimplementable as written.** It asks to
   "derive the payload-file name from a hash of `(artifact, id)`" in a runtime file
   where C-009 bans digest primitives at the source level, enforced by
   `the_runtime_computes_no_digest_c009`. The audit read the runtime closely enough
   to cite that very contract in its own *Checked and found sound* table ("The
   runtime hashes nothing at exec time") and still proposed a hash on the same
   path. The property it wanted is achievable without one.

7. **Documentation drift I did not fix, because the brief scoped `docs/**` out.**
   `docs/src/json-interface.md` carries the full `state`/`cause` table for
   `HookArmingCause` and now lacks a row for `not-registered`. The page's own next
   paragraph says "New causes may be added in a minor release under the additive policy",
   and no parity test reads it, so nothing breaks — but it is one table row of real drift.
   Recommend a follow-up adding `| not-armed | not-registered | … |`.

## Gates

Run on the final tree carrying all four fixes:

```
cargo fmt                                        clean
cargo clippy --locked --all-targets -- -D warnings   clean
cargo test --bin grim                            2918 passed; 0 failed
task verify                                      passed — 1113 acceptance tests
cd test && uv run pytest -n auto -q              1113 passed (freshly built release
                                                 binary; also run directly, because
                                                 an earlier `task verify` reported
                                                 `test:parallel` up to date from its
                                                 cache and `--force` is blocked this
                                                 session)
```

Each commit was additionally verified in isolation before being made: the P-1 tree
(2913 unit + 1112 acceptance) and the P-2 tree (2917 unit + 1112 acceptance) both
passed with their later siblings stripped out, so no commit in the sequence is
broken.

`.claude/tests/uv.lock` and `test/uv.lock` reverted; nothing staged with `git add -A`;
`commit-verified` was written by `task verify` itself, never by hand. Nothing pushed.

## Files

- `/mnt/wsl/share/dev/grimoire/grimoire/.agents/worktrees/wp8-q1/src/install/hook_registrar.rs`
- `/mnt/wsl/share/dev/grimoire/grimoire/.agents/worktrees/wp8-q1/src/oci/hook.rs`
- `/mnt/wsl/share/dev/grimoire/grimoire/.agents/worktrees/wp8-q1/src/command/status.rs`
- `/mnt/wsl/share/dev/grimoire/grimoire/.agents/worktrees/wp8-q1/src/command/hook/list.rs`
- `/mnt/wsl/share/dev/grimoire/grimoire/.agents/worktrees/wp8-q1/src/command/hook/run.rs`
- `/mnt/wsl/share/dev/grimoire/grimoire/.agents/worktrees/wp8-q1/src/api/artifact_status.rs`
- `/mnt/wsl/share/dev/grimoire/grimoire/.agents/worktrees/wp8-q1/src/install/installer.rs`
- `/mnt/wsl/share/dev/grimoire/grimoire/.agents/worktrees/wp8-q1/src/command/add.rs`
- `/mnt/wsl/share/dev/grimoire/grimoire/.agents/worktrees/wp8-q1/src/command/command_error.rs`
- `/mnt/wsl/share/dev/grimoire/grimoire/.agents/worktrees/wp8-q1/src/error.rs`
- `/mnt/wsl/share/dev/grimoire/grimoire/.agents/worktrees/wp8-q1/src/command/hook/pipeline.rs`
- `/mnt/wsl/share/dev/grimoire/grimoire/.agents/worktrees/wp8-q1/test/tests/test_hook_decline_dispatch.py`
- `/mnt/wsl/share/dev/grimoire/grimoire/.agents/worktrees/wp8-q1/.agents/security_audit_hooks.md`
- `/mnt/wsl/share/dev/grimoire/grimoire/.agents/worktrees/wp8-q1/.claude/rules/subsystem-file-structure.md`

`src/install/vendor.rs` was **not** touched: the verdict never needed exposing, because
`register_desired` reads it at the one existing seam. `src/command/hook/projector.rs` was
not touched either — P-4 is the lead's.
