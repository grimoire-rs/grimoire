# Round 3 — fix verification

Verification half of round 3: each fix applied against
[`round3-verify.md`](./round3-verify.md)'s findings, at tip **`2dea212`**
(`feat(hook)`, holds all `src/`), tree clean. No new lines of review — every
item below traces to a finding already filed.

Baselines on this tree:

```
cargo test --quiet                                → 2945 passed; 0 failed
test/ pytest (11 hook + docs + rig files)         → 144 passed
```

Method, per fix: revert it in a scratch copy of this tree and require a named
test to fail (mutation), plus an end-to-end run of the original reproduction
where one existed. `EXPECTED_UNRESERVED`-style claims were checked against the
code that writes the files, not against the doc that describes them.

## Verdict

| Finding | Fix | Verdict |
|---|---|---|
| **B2** `dispatch.json.lock` unreserved | reserved; refusal message rendered from the array | **Closed** (executed + M17) |
| **B1** the drift test did not exist | written, then rewritten to enumerate the filesystem | **Closed for install-side writes**; its promise still overreaches → **V2** |
| **W1(a)** build gate untested | `a_manifest_name_that_is_not_a_plain_name_is_refused_at_build` | **Closed** (M12) |
| **W1(b)** pre-fix artifact disarms silently | documented in hook-spec.md | **Closed** — the doc quotes the exact warning I executed |
| **W2** config-declared plain-HTTP route | `declared_insecure_hosts()` unions both scopes | **Closed** (executed A/B/C + M13) |
| **W3** issue #93's rationale falsified | note rewritten, reap split to #97, #93 comment | **Closed** (#97 exists and is the reap; #93 carries the retraction) |
| **W4** stale namespace counts | rule file rewritten without a count | **Closed** in the rule file; **hook-spec.md still contradicts itself** → **V3** |
| **W5** agreement test could not see `add` | `add::refuse_bad_binding_name` extracted, test calls it | **Closed** (M14) |
| **S1** orphaned reap comment | deleted | Closed (0 occurrences) |
| **S2** doc block on the wrong test | reattached | Closed |
| **S3** over-broad "a shell's own rule" | narrowed to exactly the three divergences I measured | Closed |
| **S4** three fixes untested | list guard + audit narrowing pinned; tier tables corrected | **Closed** (M15, M16) |
| **Question 2** — is the reserved set complete? | five names claimed | **No** → **V1** |

### Mutation matrix (scratch copy of `2dea212`, baseline 2945 green)

| Mutant | Expected | Result |
|---|---|---|
| M17 — drop `dispatch.json.lock` from the array | red | **red** ×2: `every_grim_owned_name_under_hooks_is_a_reserved_binding_name`, `read_manifest_refuses_a_traversing_record_name` |
| M11 — **my own** injection: `converge_root` also writes an unreserved `hooks/dispatch.index` | red | **red**: `every_grim_owned_name_under_hooks_…` |
| M12 — remove `SkillName::parse` from `HookManifest::validate` | red | **red**: `a_manifest_name_that_is_not_a_plain_name_is_refused_at_build` |
| M13 — `insecure: entry.insecure` (W2 reverted) | red | **red**: `a_second_entry_downgrading_the_same_host_strips_the_implicit_grant` |
| M14 — neuter the reserved half inside `add::refuse_bad_binding_name` | red | **red**: `the_add_path_and_the_install_path_agree_on_every_binding_name` |
| M15 — remove `hook list`'s grammar guard | red | **red**: `read_manifest_refuses_a_traversing_record_name` |
| M16 — `unlogged_mutator_outcome` always reports a discarded rewrite | red | **red**: `only_a_mutator_that_actually_rewrote_reports_a_discarded_rewrite` |
| M10 — a new unreserved file written by the **runtime** path | ? | **green** → V2 |
| control | green | green (2945) |

Every fix that was "untested" last round now has a test that dies when the fix
is reverted. M14 is the one worth naming: the agreement test used to survive the
deletion of `add`'s checks, and now fails when the extracted function is
neutered, so the claim its doc makes is finally true.

---

## Warn

### V1 — the reserved set is still incomplete, and two new docs say the opposite

Re-derived from the code rather than from the list. Everything grim itself writes
**directly** under `$GRIM_HOME/hooks/`:

| Name | Written by | A `SkillName`? | Reserved? |
|---|---|---|---|
| `dispatch.json` | `dispatch_path` → `converge_root` | yes | ✓ |
| `dispatch.json.lock` | the table's advisory-lock sidecar (`advisory_lock.rs:201`) | yes | ✓ (B2) |
| `root-key` | `root_key_path` (mint / read) | yes | ✓ |
| `bin/` (+ `bin/grim-hook`) | `hook_launcher::launcher_dir` | yes | ✓ |
| `payload/` | `payload_relative`, workspace scope | yes | ✓ |
| `hook_audit.jsonl`, `…​.jsonl.1` | `run.rs:270` + rotation | **no** — underscore | n/a, safe |
| `.tmp*` | `atomic_write` → `NamedTempFile::new_in(parent)` | **no** — leading dot | n/a, safe |
| **`payload-<pid>-<slot>.json`** | `pipeline.rs:1082` `write_payload_file` | **yes** | ✗ **not reserved** |
| `<artifact>/` (global scope) | materialization | — | the artifact's own, by design |

Only one advisory lock touches this directory, so there is exactly one sidecar —
the fix is complete on that axis.

The last row is the gap, and it is asserted away twice in new prose:

* `src/oci/hook.rs` (the `RESERVED_ARTIFACT_NAMES` doc, final paragraph):
  “the second is written **inside a payload directory** rather than at the root
  of `hooks/`”.
* `src/install/hook_dispatch.rs` (`EXPECTED_UNRESERVED`'s doc): “the transient
  `payload-<pid>-<slot>.json` files **live inside a payload directory**, so
  neither is representable or reachable as a binding name”.

Both halves are false, and the code says so twice over. `write_payload_file`'s
own doc: “**Beside the audit trail, inside the `0o700` hooks directory** … **Never
inside `payload_dir`**, which is the materialized artifact tree whose content hash
`grim status` compares”, and the path is `invocation.audit.path().parent()`
(`pipeline.rs:619`) — the parent of `hooks/hook_audit.jsonl`, i.e. `hooks/`
itself. `.claude/rules/subsystem-file-structure.md` (rewritten for W4) also has
it right: it lists “transient `payload-<pid>-<slot>.json` envelopes” as part of
the `hooks/` namespace. So the repo now states both, and the two statements that
are wrong are the two that a maintainer consults when deciding whether a name
needs reserving.

Executed, one run (`scratchpad/envelope_location.sh` — a `payload = "file"`
observer whose handler records `$GRIM_HOOK_PAYLOAD` and lists its parent):

```
GRIM_HOOK_PAYLOAD=…/env1/home/hooks/payload-1864835-0.json
ls hooks dir: bin dispatch.json file-probe hook_audit.jsonl
              payload-12345-0.json payload-1864835-0.json root-key
```

The same run binds a second hook to `payload-12345-0.json` — an
envelope-shaped name — and it is **accepted**:

```
hook  payload-12345-0.json  …/home/hooks/payload-12345-0.json  installed  claude (observer)
$ ls …/home/hooks
bin  dispatch.json  file-probe  payload-12345-0.json  root-key
```

so the binding namespace and the envelope namespace are the same directory, and
the name is representable in both.

**Severity of the collision itself stays low**, exactly as round 2 judged it: the
name carries the writing process's pid, so an attacker cannot target it, and the
consequence of a hit is one `payload = "file"` hook failing to receive its
envelope for one invocation (fail-open, no verdict) — not the machine-wide
disarm that made `root-key` and `dispatch.json.lock` Blocks. What makes this a
Warn is the pair of false claims, in the one place designed to be read by the
next person adding a file there, on a branch where a false claim of exactly this
shape has now shipped twice.

Two ways out, either acceptable:

1. **Move the envelopes into `hooks/tmp/`** and reserve `tmp`. They are transient
   and unreferenced across invocations, so there is no state to migrate and
   nothing additive to preserve; it dissolves the class rather than documenting
   it, and it also removes them from the `hooks/` listing the B1 test walks.
2. **Keep them where they are and make both docs true**: they live at the root of
   `hooks/`, `payload-<pid>-<slot>.json` *is* representable, a collision costs one
   hook one firing, and that cost is accepted because the pid is unguessable.
   They cannot go in `EXPECTED_UNRESERVED` (the names are dynamic), which is
   precisely why the note has to be honest instead.

### V2 — the drift test is blind to runtime-side writes, and one doc promises otherwise

The rewritten test is a real improvement and I could not fool it on the axis it
covers: **M11**, a new unreserved `hooks/dispatch.index` written by
`converge_root` that nothing told the test about, fails it. Observing the
namespace both under the lock and after it releases is the right call — that is
what catches the sidecar.

But the provocation is `root_token` + `hook_launcher::generate` +
`converge_root`, so the namespace it can see is install's dispatch-side writes.
**M10**: change `write_payload_file` to a fixed name — `dir.join("envelope.json")`,
an unreserved name grim then writes at the root of `hooks/` on every
file-transport invocation — and the suite stays **green, 2945 passed**. The same
would hold for a new file written by the audit trail, by materialization, or by
any future runtime path.

`hook_dispatch`'s own doc is honest about this (“the provocation covers install's
**dispatch-side** writes; `payload/` is created by materialization, which this
test does not run”). The doc a reader meets first is not:

* `src/oci/hook.rs`: “So it fails for the next file grim puts there **whether or
  not anyone remembers to tell it about the file**.” True for install's
  dispatch-side writes; false for the rest, as M10 shows.
* `catalog/.../hook-spec.md:339`: “fails the build for a **layout constant** that
  is not reserved”. The test no longer looks at layout constants at all — that
  was the version you replaced.

This is the same over-claim shape as B1 (a promise wider than the guard), one
notch smaller because the guard now exists and does work. Scope the sentence to
what the provocation covers, or widen the provocation — firing one
`payload = "file"` hook inside the test would extend it to the runtime side and
would immediately surface V1.

### V3 — hook-spec.md contradicts itself on the count, and quotes a message `grim build` does not print

`catalog/skills/grim-authoring/references/hook-spec.md`, a published catalog
skill under `catalog/README.md`'s drift-review duty:

* `:308` — “On top of that, **five** names are refused outright: `bin`,
  `dispatch.json`, `dispatch.json.lock`, `payload`, and `root-key`.” Correct.
* `:354` — “The reserved check is exact string equality against those **four**.”
  Stale, 46 lines below the corrected sentence, in the same section.
* `:423` — the **build**-pitfalls table (“Every row below fails `grim build` with
  **exit 65**”) quotes `binding_name_refusal`'s wording:
  `'bin' is reserved: grim's own launcher, dispatch table (and its lock), payload root and machine key live at …`
  `grim build` prints `ReservedArtifactName`'s wording instead — executed on this
  binary:

  ```
  $ grim build …/bin --kind hook
  hook artifact name 'bin' is reserved: 'bin' names part of grim's own hook launcher under $GRIM_HOME/hooks/ — rename the artifact
  $ echo $?
  65
  ```

  which is exactly what `:345` of the same file shows for the same case. Updating
  the row's name list without its message re-introduced the defect round-2 F5
  filed against this very table.

---

## Verified closed, with the evidence

* **B2.** Executed on a fresh `GRIM_HOME`: a hook bound as `dispatch.json.lock`
  is `skipped` before materialization, the sibling `write-guard` arms as
  `claude (gatekeeper)`, `dispatch.json` is written. The refusal message is
  rendered from the array — `$GRIM_HOME/hooks/{bin,dispatch.json,dispatch.json.lock,payload,root-key}`
  — so that second enumeration cannot fall behind, and M17 shows the array and
  the test are coupled in both directions.
* **B1**, on its own axis. M11 (above). The escape hatch's asymmetry is right:
  `EXPECTED_UNRESERVED` exempts *unreserved* names, so a forgotten entry fails.
  Not asserting the reverse direction remains correct.
* **W2.** My round-3 case A, re-executed unchanged: a project entry carrying
  `insecure = true` for `127.0.0.2:5050` now **strips** the ordinary global
  namespaced entry's implicit grant — `skipped`, “this registry has not been
  trusted for hooks and there is no terminal to ask on” — while control B (no
  downgrade → HTTPS attempted, fetch fails) and control C (flag on the granting
  entry → skipped) behave as before. `declared_insecure_hosts` reads
  `self.registries` (both scopes, `oci` entries only, host **with** port), which
  mirrors `command::declared_insecure_hosts`; `index` entries are correctly
  excluded, since they carry no OCI transport. Loopback still grants, so the rig
  and the acceptance suite are unaffected — 144 hook/docs/rig acceptance tests
  pass.
* **W1(b).** hook-spec.md's new paragraph quotes the warning text I executed last
  round verbatim, and says the plain thing a reader needs: rename the manifest
  `name` and republish; the binding name in `grimoire.toml` need not change.
* **W3.** #97 exists — *“Dispatch table accumulates a root per abandoned
  checkout, with no sound reap”* — #93 carries a comment retracting the falsified
  rationale, and the withdrawal note now says the table only grows, that the 80 %
  warning “does not prevent it”, that #93 is the writer-side refusal and #97 the
  reap, and “do not cite #93 for it”. All four claims check out.
* **W4**, in the rule file. `subsystem-file-structure.md` no longer states a
  count, lists the sidecar, `payload/`, the audit trail and the transient
  envelopes, and states the rule that matters: adding a file here means adding
  its name to `RESERVED_ARTIFACT_NAMES` unless it is unrepresentable as a binding
  name.
* **S4's doc half.** Both tier tables now carry the conditional — `audit.rs:55`
  (“`RewriteDiscardedUnlogged` when it rewrote something, `Completed` when it did
  not”) and `audit.rs:493-497` — and `pipeline.rs:513-519` matches.
* **S1/S2/S3.** The orphaned reap comment is gone; the traversal doc block sits
  on `a_binding_name_that_is_not_a_plain_name_is_refused` again; and
  `expand_payload_dir`'s doc now names exactly the three divergences I measured
  against bash (`'…'`, `\$…`, `${…:-x}`) and why none is a risk.

## On `payload-<pid>-<slot>.json` not being reserved — challenged, as invited

Not reserving it is the right call; **the reasons given for it are not.** A
prefix reservation is indeed a different mechanism, and reserving the literal
`payload-12345-0.json` would be theatre. What does not follow is the claim that
the name is unreachable — it is reachable (executed above), the file does sit in
the binding namespace, and the honest statement is that the collision is
unguessable and cheap. Fix the prose, or move the files (V1); do not extend the
array.
