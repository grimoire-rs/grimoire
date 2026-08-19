# Round 3 — verification

Final round of the bounded review loop. Scope: the fixes applied in response to
`round2-correctness.md` and `round2-security.md`.

**The branch was rewritten under me mid-review.** I started against
`f88b346..baf0297` (6 commits); the branch was then squashed to 5 commits and
HEAD moved three times while I worked (`f6c4977` → `885efe2` → `eca0ddc`). The
**working-tree content never changed** during that window (`cargo build
--release` was a 0.14 s no-op across the rewrite; every file I cite I re-read
after it), so every finding below is stated against the working tree as it
stands at `eca0ddc`, with file:line re-verified after the squash. One
consequence worth knowing: the squash also **deleted my first copy of this
file**, so a further rewrite may do the same — a copy lives at
`/tmp/claude-1000/-mnt-wsl-share-dev-grimoire-grimoire/dd45c0cc-03cc-49e4-a3f0-c74b48f12672/scratchpad/round3-verify.md`.

Baselines, all green before any finding was written:

```
cargo test --quiet                                  → 2940 passed; 0 failed
cargo clippy --all-targets --quiet                  → clean
cargo fmt --check                                   → clean
test/ pytest test_hook_run_runtime, test_hook_decline_dispatch,
      test_golden_pre_hooks, test_bundle_hook_members,
      test_docs, test_manual_rig                    → 92 passed
```

Evidence was produced with `target/release/grim` against the rig registry on
`localhost:5050` (and `127.0.0.2:5050`, the same registry on a loopback address
that is *not* in `is_loopback`'s set), plus a **pre-fix** `grim` built from
`f88b346` in a scratch extraction, and a scratch copy of the current tree used
for mutation testing. No repo file was modified except this report.

## Verdict per round-2 finding

| Round-2 finding | Claimed fix | Verdict |
|---|---|---|
| **S2-1** (Block) `root-key` unreserved | added to `RESERVED_ARTIFACT_NAMES`, message updated, new drift test | **Refusal works** (executed). **The drift test does not exist** → B1. **The set is still incomplete** → B2 |
| **F2 + S2-3** reap unsound | withdrawn entirely | **Withdrawal is correct and clean** — one orphaned comment (S1) and one stale issue (W3) |
| **F1** reap reported at `info` | moot once withdrawn | Cleared |
| **F3** vacuous reap test | test deleted with the reap | Cleared |
| **F4** braceless `$GRIM_HOOK_DIR` boundary | expand unless `[A-Za-z0-9_]` follows | **Correct** — agrees with bash on every delimiter I tried; guarded by a test (M2) |
| **F5** build accepts a name no consumer does | `SkillName::parse` in `HookManifest::validate` | **Works** (executed), docs row now true, **no test** → W1 |
| **S2-2** (Warn) env var defeats condition 5 | `entry.insecure \|\| locator_is_plain_http(locator)` | **Env route closed** (executed). **Config route still open** (executed) and the new test pins the helper, not the wiring → W2 |
| **F6** oversize table write | not taken; deferred to issue #93 | Deferral rationale is now false → W3 |
| **S1** `hook list` path guard | grammar gate in `read_manifest` | Correct shape, no test (S4) |
| **S2** `add` is a fourth spelling | agreement test | **The test does not observe `add`** → W5 |
| **S3** unreachable glob row | comment | Cleared |

## Mutation matrix — would each fix's own tests notice its reversion?

Run in a scratch copy of the current tree (baseline 2940 green; `find -name
'*.rs' -exec touch` before each run, because a `tar`-preserved mtime silently
made cargo reuse a stale test binary the first time — worth knowing if you
repeat this).

| Mutant | Result |
|---|---|
| M1 — drop `"root-key"` from `RESERVED_ARTIFACT_NAMES` | **FAILS** `oci::hook::tests::a_binding_name_that_is_not_a_plain_name_is_refused` — the literal-list rewrite works |
| M2 — F4 boundary back to `/`-or-end | **FAILS** `pipeline::tests::the_payload_dir_token_expands_in_every_argv_element` |
| M3 — remove `SkillName::parse` from `HookManifest::validate` (F5) | **green, 2940 passed** |
| M4 — `insecure: entry.insecure` (S2-2 reverted) | **green, 2940 passed** |
| M5 — remove the `hook list` grammar guard (S1) | **green, 2940 passed** |
| M6 — set `RewriteDiscardedUnlogged` unconditionally again | **green, 2940 passed** |
| M7 — delete `add`'s reserved-name check on **both** of its paths | **green, 2940 passed** |
| control (pristine) | green, 2940 passed |

No acceptance test covers M3, M4 or M7 either (grepped: no acceptance test
builds a hook with an unusable name, none exercises the trust predicate with a
downgraded transport, and `test_hook_decline_dispatch.py`'s P-2 test
deliberately hand-writes the binding into the config rather than going through
`grim add`).

---

## Block

### B1 — the drift guard the `root-key` fix rests on was never written

`src/oci/hook.rs:161-172` (the doc), commit message of the reserve-root-key
commit.

`RESERVED_ARTIFACT_NAMES`'s new doc closes with:

```
/// The list cannot be derived here without inverting the module direction
/// (`oci` must not depend on `install`), so the drift is prevented from the
/// other side instead: `hook_dispatch`'s
/// `every_grim_owned_name_under_hooks_is_a_reserved_binding_name` asserts each
/// layout constant is present, and it fails the build for the next file grim
/// puts under `hooks/`.
```

That test does not exist:

```
$ grep -rn 'every_grim_owned' . --include='*.rs' --include='*.py' --include='*.md'
src/oci/hook.rs:170:/// `every_grim_owned_name_under_hooks_is_a_reserved_binding_name` asserts each

$ git log --all --oneline -S 'every_grim_owned_name_under_hooks_is_a_reserved_binding_name'
<only the commit that added the doc line>
```

So the one mechanism the fix offers against the failure mode that *produced*
S2-1 — a literal list in `oci::hook` tracking a layout owned by
`install::hook_dispatch` — is absent, and in its place is a doc comment and a
commit message that both assert it is present. That is worse than not having it:
the next person to add a file under `$GRIM_HOME/hooks/` reads this paragraph and
concludes the build will catch them. It will not — B2 is the proof, and B2 is a
name that was *already* missing when this doc was written.

Fix: write the test the doc names, in `hook_dispatch` (which may depend on
`oci`), over `DISPATCH_FILE`, `PAYLOAD_DIR`, `ROOT_KEY_FILE`,
`hook_launcher::LAUNCHER_DIR` — and the lock sidecar name from B2. The reverse
direction the brief mentions ("nothing is reserved which is not grim's own") is
**not** worth asserting: `hook_audit.jsonl` and `dispatch.json.lock` are grim's
own and are not reserved, so a bidirectional test would either fail immediately
or have to encode the exceptions, which re-creates the drift it is meant to
prevent. Assert one direction: every grim-owned name is reserved.

### B2 — the reserved set is still incomplete: `dispatch.json.lock` reproduces S2-1

`src/oci/hook.rs:173` (four names), `src/lock/advisory_lock.rs:201-208` (the
sidecar is `<file>.lock`, the **full** file name plus the suffix),
`src/install/hook_dispatch.rs:206` (global payload dir is `hooks/<name>`).

`$GRIM_HOME/hooks/dispatch.json` is guarded by an advisory lock whose sidecar is
`$GRIM_HOME/hooks/dispatch.json.lock` — a **fifth** grim-owned entry directly
under `hooks/`. `dispatch.json.lock` is a valid `SkillName` (lowercase, dot
separators, no adjacent separators) so the grammar gate passes it, and it is not
in the reserved array. Executed, fresh `GRIM_HOME`, global config binding a rig
hook as `dispatch.json.lock` plus a real `write-guard` gatekeeper
(`scratchpad/repro_lock_sidecar.sh`):

```
$ grim --global install < /dev/null
WARN grim::install::hook_registrar: dispatch table write failed; nothing armed
     error=dispatch table I/O failure: Is a directory (os error 21)
Kind  Name                Target                          Status     Armed
hook  dispatch.json.lock  …/hooks/dispatch.json.lock      installed  —
hook  write-guard         …/hooks/write-guard             installed  —

$ ls -la …/hooks
drwxr-xr-x 2 … dispatch.json.lock     <-- a DIRECTORY where the lock sidecar goes
-rw------- 1 … root-key
drwx------ 2 … bin
$ ls …/hooks/dispatch.json
ls: cannot access …: No such file or directory        <-- no dispatch table at all

$ grim --global install < /dev/null        # second run, identical failure
$ grim --global status
hook  write-guard  …  not-armed  claude: not-registered
```

One artifact, chosen by a bundle (T1 — a bundle member's binding name never
passes through `grim add`), and **every hook on the machine stops arming for as
long as it is installed**: `AdvisoryFileLock::try_acquire` cannot open the
sidecar path (`EISDIR`), `converge_root` returns `DispatchError::Io`, and no
table is written. Same class as S2-1, same invariants (I3, I5).

Two differences from S2-1, both in the milder direction, which is why I would
accept **Warn** if the owner prefers it — I file **Block** because round 2 rated
the identical shape Block, the fix is one array element, and this is the exact
question the brief asked ("is the reserved set now complete?"):

* **Recoverable.** `grim uninstall --global hook dispatch.json.lock` succeeded in
  my run and removes the directory; `root-key` on a fresh machine destroyed the
  key path irrecoverably by comparison.
* **Not silent.** A `warn!` fires on the install path and `grim status` reports
  `not-armed / not-registered` — though neither names a directory collision, so
  the cause is not discoverable from the output.

Also still representable and unreserved, and worth naming while the array is
open: `payload-<pid>-<slot>.json` (the transient envelope files,
`pipeline.rs`), which cost one hook one firing on a pid collision. Safe only
incidentally: `hook_audit.jsonl` and `hook_audit.jsonl.1` are unrepresentable as
a `SkillName` because of the underscore.

---

## Warn

### W1 — F5 works, has no test, and does change what a pre-fix artifact does

The gate itself is correct and reachable, executed against both binaries:

```
name         NEW (HEAD)                                              OLD (f88b346)
shell-guard  exit 0 built                                            exit 0 built
bin          exit 65 …'bin' is reserved…                             exit 65 (same)
root-key     exit 65 …'root-key' is reserved…                        exit 0 BUILT
my_hook      exit 65 hook artifact name 'my_hook' is not usable:     exit 0 BUILT
             skill name 'my_hook' must contain only lowercase…
MyHook       exit 65 (same shape)                                    exit 0 BUILT
```

`hook-spec.md:397`'s message is now quoted verbatim-correct, and all four hook
artifacts in the repo still build (`catalog/hooks/{tool-call-logger,
command-guard}`, `test/manual/catalog/hooks/{tool-logger,write-guard}` — exit 0
each). Two problems remain.

**(a) No test guards it** (M3 green). Principle 2 asks for a regression test per
fix, and this one is a published-format gate that a future refactor of
`validate_reserved_name` could drop invisibly. A three-line unit test on
`HookManifest::validate` with `name = "my_hook"` closes it.

**(b) The install-time re-check now disarms a pre-fix artifact.** Executed
end-to-end: a hook whose *manifest* name is `my_hook`, published with the
pre-fix binary and bound under the perfectly legal binding name `shell-guard`,
was **installed and armed** before the fix and now installs without arming.

```
# published by the f88b346 binary to localhost:5050/r3probe/hooks/legacy-hook:1.0.0
OLD install:  hook shell-guard … installed  claude (observer)   → dispatch row present
NEW install:  WARN hook 'shell-guard' is not armed: its installed hook.toml does not
              satisfy the rules `grim build` enforces, so it was published without them:
              hook artifact name 'my_hook' is not usable: …
              hook shell-guard … installed  —                    → no dispatch table
```

I do **not** call this a Principle 9 break: the hook kind is gated off and absent
from 0.13.0 (the reasoning `RESERVED_ARTIFACT_NAMES`'s own doc uses to justify
appending a name), so there is no released surface to freeze, and the degrade is
fail-closed with the most legible message in the feature. Worth a line in the
changelog/hook docs anyway, because anyone who built a hook against a pre-fix
build of this branch sees a working hook stop arming with no config change.

### W2 — S2-2 is half closed: the *config*-declared plain-HTTP route still arms with no prompt

`src/hook/policy.rs:63-73` uses `registry_client::plain_http_hosts()` — loopback
plus `GRIM_INSECURE_REGISTRIES`. The **effective** transport set is
`plain_http_hosts_with(declared_insecure_hosts(…))` (`src/command.rs:593,
656-680`), which additionally unions the *host* of every `[[registries]]` entry
that declared `insecure = true`, **across both scopes, project included**. Round
2's suggested fix said `plain_http_hosts_with(&config_insecure_hosts)`; the
`_with` half was dropped, and `entry.insecure` only covers the entry being
classified — not another entry naming the same host.

Executed three ways against `127.0.0.2:5050` (reachable, and deliberately *not*
in `is_loopback`'s `{localhost, 127.0.0.1}` set), `< /dev/null` so no prompt is
possible (`scratchpad/s22_config_route.sh`):

| Case | Global entry (the granting one) | Project entry (attacker-carried) | Result |
|---|---|---|---|
| **A** | `oci = "127.0.0.2:5050/grimoire"`, no flag | `oci = "127.0.0.2:5050"`, `insecure = true` | **ARMED** `write-guard` (gatekeeper), no prompt |
| **B** | same, no flag | *none* | fetch attempts `https://127.0.0.2:5050/…` and fails — proves A really was plain HTTP |
| **C** | same **+ `insecure = true`** | `insecure = true` | `skipped`, "not been trusted for hooks and there is no terminal to ask on" |

A committed `grimoire.toml` is grim's own canonical T3 vehicle — the same
argument the fix's comment makes about `.envrc` — so the attack S2-2 describes
survives verbatim with `.envrc` swapped for the repo's own config: a cloned
repository downgrades the transport for a host, the victim's ordinary namespaced
global entry keeps its implicit grant, and a gatekeeper from that host arms
without a prompt over plain HTTP.

The half that *was* fixed is genuinely fixed — verified separately
(`scratchpad/s22_env_route.sh`): same shape with the downgrade coming from
`GRIM_INSECURE_REGISTRIES` and no `insecure` anywhere now reports `skipped`.
And the rig/acceptance concern is answered: `is_loopback` strips the port, so
`localhost:5050` still grants and nothing in the rig or the suite changed (92
hook acceptance tests green).

**The new test pins the wrong level.**
`policy::tests::the_plain_http_probe_reads_the_host_out_of_every_locator_shape`
tests `locator_is_plain_http` in isolation — good coverage of the host parsing
(I found no false positive in its rows or mine: scheme, port, path, trailing
slash, and the `localhost.evil.dev` near-miss all classify correctly) — but M4
shows that reverting the **wiring** (`insecure: entry.insecure`) leaves the whole
suite green. The fix's load-bearing line has no test.

### W3 — F6's deferral rationale rested on the reap that was just withdrawn

`src/install/hook_dispatch.rs:810-836` (the withdrawal note) says the design
question is "tracked in issue 93". Issue #93 is a different question — it is
**F6**, the oversize write — and its body now contains two statements the
withdrawal falsified:

> Two mitigations landed with the hooks work … `reap_dead_roots` drops roots
> whose workspace no longer exists, so the table no longer grows without bound in
> the normal case. … Reaching the cap now requires many *live* roots, which is
> remote but not impossible on a shared `$GRIM_HOME`.
> … the growth fix removes the realistic path to the cap. Splitting them keeps
> the merge reviewable.

With the reap gone, the table again only grows, so the argument that justified
deferring the writer-side check is void — and #93's suggested fix is now the
*only* thing standing between an accumulating table and the total disarm at
`read_table`'s cliff. Nothing in the code or the issue records that. Two
concrete items: point the withdrawal note at an issue that actually holds the
reap question (or open one), and update #93's "why it was not fixed" section.
The comment's own phrase "the growth is bounded by the warning below" is also
not true as written — a `warn!` is a diagnostic, not a bound.

The 80 % warning itself is intact and reachable: `converge_root:918-926` is
unchanged, fires on any write whose serialized table exceeds
`MAX_TABLE_BYTES * 8 / 10`, and the constants have no other consumer.

### W4 — the docs that caused S2-1 still say three

Two enumerations of grim's `hooks/` namespace were not updated with the array,
which is precisely the drift shape that produced the Block:

* `catalog/skills/grim-authoring/references/hook-spec.md:396` — "Artifact named
  `bin`, `dispatch.json` or `payload`". `root-key` is missing, and `grim build`
  does refuse it (executed above, exit 65). This is a **published catalog
  skill**, i.e. under `catalog/README.md`'s drift-review duty.
* `.claude/rules/subsystem-file-structure.md:151` — "`payload` is therefore a
  **third** `RESERVED_ARTIFACT_NAMES` entry" (there are four, and `root-key` is
  never named as reserved anywhere in that file), and `:190` — "**Three** things
  grim writes for hooks live under `$GRIM_HOME/hooks/`" over a three-row table.
  There are at least six (the three listed, plus `root-key`'s neighbours:
  `dispatch.json.lock`, `hook_audit.jsonl`, `hook_audit.jsonl.1`, the transient
  `payload-*.json`, and the `payload/` tree). This file is auto-loaded on every
  `src/**` edit, so it is the sentence the next author will trust.

### W5 — the add-vs-install "agreement" test cannot observe `add`

`src/oci/hook.rs:1642-1685`. The test re-spells `add`'s logic inside itself:

```rust
let add_refuses = crate::skill::SkillName::parse(name).is_err() || is_reserved_binding_name(name);
assert_eq!(add_refuses, binding_name_refusal(name).is_some(), …);
```

`add`'s checks are inline `if` blocks in `command::add` (`add.rs:237-250` and
`:520-529`), so nothing in this test reaches them. M7 — deleting the
reserved-name check from **both** of `add`'s paths — leaves the suite green,
including this test. Its doc claims the opposite:

> This pins the agreement instead of the implementation, so either site may be
> rewritten as long as both still answer alike.

As written it pins `binding_name_refusal` against a hand-copy of `add`'s current
logic, which is a tautology that will keep passing after `add` diverges. To make
the claim true, either extract `add`'s two checks into a small named function the
test can call, or assert through `grim add`'s observable behaviour in the
acceptance suite (there is currently no acceptance coverage of `grim add --name
bin` for a hook either).

---

## Suggest

### S1 — an orphaned comment is the only residue of the withdrawn reap

`src/install/hook_dispatch.rs:892-894`, now sitting above `let desired = …`:

```rust
// Under the same lock and folded into the same atomic write as the converge:
// a separate lock/write pass would double the write amplification and open a
// window where a concurrent install re-adds what this one just dropped.
```

That was the reap's justification — nothing here re-adds or drops anything now.
Otherwise the removal is clean: `converge_root`'s body is byte-identical to its
pre-reap form (`diff` against `eeca22f^` shows only this comment and the retained
80 % warning), `DispatchWrite::Unchanged`'s doc is honest again ("The table
already said exactly this — no bytes written"), no constant is left dead, both
reap tests are gone, and no doc anywhere else in the repo mentions a dispatch-root
reap (the `reap` hits are all `install/prune.rs`, unrelated). The withdrawal's
cited justification checks out too — `adr_install_state_portability.md` is titled
"Portable install state for shared GRIM_HOME / devcontainers", so the layout it
protects is real and documented.

### S2 — a doc block landed on the wrong test

`src/oci/hook.rs:1620-1643`: the new agreement test was inserted **between** the
existing doc comment and the function it documents. The paragraphs about
traversal shapes, "a blocklist has to anticipate the next shape", and the wave-8
reproduction now document
`the_add_path_and_the_install_path_agree_on_every_binding_name`, and
`a_binding_name_that_is_not_a_plain_name_is_refused` — the test they were written
for, and the one carrying the security rationale — has no doc at all.

### S3 — F4's rule over-expands only where a shell has no say

Verified the implemented rule against bash for every boundary I could think of
(`scratchpad/f4/`, the function body copied verbatim): `:`, `.`, `-`, `}`, `%`,
`$`, `"`, a following non-ASCII byte, and the braced form all agree, and
`$GRIM_HOOK_DIRECTORY` / `_SUFFIX` / `2` are correctly left intact. Two
divergences, both harmless and worth a sentence in the doc rather than a code
change:

* `'$GRIM_HOOK_DIR'` and `\$GRIM_HOOK_DIR` **expand** here where a shell would
  suppress. Quoting and escaping have no meaning in exec form (the bytes reach
  `execve` verbatim), and the substituted value is grim-derived, so nothing is
  at risk — but the doc's "that is a shell's own rule" is now the only claim in
  the function that is not literally true.
* `${GRIM_HOOK_DIR:-fallback}` is left literal (a shell expands it). Fail-visible,
  and parameter expansion was never in the documented contract.

### S4 — three more fixes ship without a test

M5 (`hook list`'s grammar guard), M6 (the `RewriteDiscardedUnlogged`
narrowing) and W1(a) all leave the suite green when reverted. M5 and M6 are
cheap to pin: `read_manifest` can be exercised with a hand-written record name,
and the audit narrowing is a pure decision over `response.updated_input`.

While in M6's neighbourhood: two tier tables now over-claim, since a mutator
that returns no rewrite reports `Completed` rather than
`RewriteDiscardedUnlogged` — `src/hook/audit.rs:55` and `:489-493` ("`mutator` —
spawn, then **discard the rewrite** … and report
`AuditOutcome::RewriteDiscardedUnlogged` with the verdict the hook actually
returned"), and the same row at `src/command/hook/pipeline.rs:496`.

---

## Attacked and held (evidence that convinced me)

* **S2-1's refusal.** Executed on a fresh `GRIM_HOME` with `[hooks] root-key =
  …`: `WARN hook 'root-key' not installed: 'root-key' is reserved: … live at
  $GRIM_HOME/hooks/{bin,dispatch.json,payload,root-key} …`, the key file
  survives as a 32-byte `0o600` regular file, `dispatch.json` is written, and the
  sibling `write-guard` arms as `claude (gatekeeper)`. The message's namespace
  enumeration matches the array.
* **The literal-list rewrite of the reserved test.** M1 proves it: dropping
  `root-key` from the array now fails a test, where the array-driven loop it
  replaced could not.
* **F4.** M2 proves the new rows are load-bearing; the bash comparison above
  proves the rule is the right one.
* **F5's non-regression.** Every hook artifact in the repo builds (exit 0), and
  `test_manual_rig.py` now parametrizes the rig's two hooks through `grim build`,
  so a rig hook with an unusable name would fail there.
* **S1's shape.** `read_manifest`'s bail is consumed per subject by a
  warn-and-continue arm (`list.rs:220-234`), so one bad record cannot fail the
  whole `grim hook list` — degrade, not error (I3).
* **Collapsed capability predicate, `status` delegation, dead `From` impls.**
  The removals compile and the full suite passes, which is the proof for
  dead-code claims; `hook_registrar`'s "see the section at the end of this doc"
  cross-reference does resolve to a real section.
* **Docs claims I could check.** `grim publish`'s kind order really is
  skills → rules → agents → mcp → **hooks** → bundles
  (`publish.rs:1876-1891`); the registered one-liner really does contain `s=$?`
  and `case … esac`, so `clients.md#gap-codex-shell`'s grim-side half is accurate;
  `json-interface.md`'s `not-registered` row and message match
  `artifact_status.rs` (and `test_docs.py::test_every_hook_arming_cause_is_documented`
  now mechanizes that); issues #92 and #93 exist and are open.

## Not defects (checked and dismissed)

* **The `hook list` guard rejecting a legitimate record.** It is exactly the
  install seam's predicate, so any name install accepts, `list` accepts.
* **`locator_is_plain_http`'s parsing.** No false positive found across bare
  host, `host:port`, namespaced, `scheme://`, trailing slash, uppercase
  (`eq_ignore_ascii_case` is *wider* than `HttpsExcept`'s exact match, so an
  uppercase host classifies as plain HTTP while the transport stays HTTPS — a
  fail-closed refusal, and unreachable in practice since a locator's host is
  lowercased everywhere it is compared) and the `localhost.evil.dev` near-miss.
* **The reap's absence breaking anything.** `converge_root` is pre-reap-identical
  and the two removed tests were the only observers.
* **`HookArmingInputs.workspace`.** Plumbed and unused by the `Hook` arm, as its
  own doc says; `status` reports nothing differently (round 2 cleared this and
  the delegation did not change it).

---

## Addendum — fixes applied to the working tree *during* this review

The author began fixing these findings while I was still writing, so by the time
this file was saved the tree had moved again. What I could verify, I did.

**B1 + B2 — fixed and verified.** `RESERVED_ARTIFACT_NAMES` is now five names
(`dispatch.json.lock` added), the refusal message renders the namespace *from the
array* instead of re-typing it, and the drift test the doc promised now exists at
`src/install/hook_dispatch.rs:967`
(`every_grim_owned_name_under_hooks_is_a_reserved_binding_name`). It is the right
shape: it acquires the real lock and **observes** the sidecar's name from
`read_dir` rather than restating it, so the next name grim writes there is caught
without anyone having to remember it. Verified non-vacuous by mutation on a
scratch copy (baseline 2941 green):

| Mutant | Result |
|---|---|
| M8 — drop `dispatch.json.lock` from the array | **FAILS** `every_grim_owned_name_under_hooks_is_a_reserved_binding_name` |
| M9 — drop `root-key` from the array | **FAILS** it *and* `a_binding_name_that_is_not_a_plain_name_is_refused` |

And end to end, re-running B2's own reproduction against a rebuilt binary:

```
WARN hook 'dispatch.json.lock' not installed: 'dispatch.json.lock' is reserved: grim's own
     launcher, dispatch table (and its lock), payload root and machine key live at
     $GRIM_HOME/hooks/{bin,dispatch.json,dispatch.json.lock,payload,root-key} …
hook  dispatch.json.lock  …  skipped    —
hook  write-guard         …  installed  claude (gatekeeper)     ← dispatch table written
```

**W2 — a completion is in flight; it looks right and I could not verify it on a
stable tree.** `locator_is_plain_http` now takes the authored set
(`plain_http_hosts_with(declared_insecure)`), fed by a new
`HookPolicy::declared_insecure_hosts` that mirrors `command::declared_insecure_hosts`
over both scopes' entries — which is the level the finding asked for — and a new
unit test `a_second_entry_downgrading_the_same_host_strips_the_implicit_grant`
encodes my executed case A directly. I saw that test pass in one snapshot. I was
**not** able to record a green full-suite run: over the following minutes the tree
went through a state that did not compile, a state where that test failed, and a
state where it passed. Two things for whoever lands it:

* `cargo fmt --check` currently flags a stray blank line at `src/hook/policy.rs:72`
  (and, in an earlier snapshot, wrapping in the new dispatch test). Run
  `task verify` on a quiesced tree — nothing in this addendum substitutes for it.
* **S2 recurred in the same edit.** `declared_insecure_hosts` was inserted between
  `authored()`'s doc comment and `authored()`, so lines 174-177 ("The borrowed
  `AuthoredRegistry` view `decide` takes… Never pre-normalized…") now document
  `declared_insecure_hosts`, and `authored()` is undocumented. Same defect as S2 in
  `oci/hook.rs:1641`, twice in one round — worth a habit rather than a fix: add the
  new item *after* the neighbour's `fn`, not before its doc.

**Still open at my last check** (`6d9310a` plus the in-flight edits): W1 (F5 has no
test; the pre-fix-artifact disarm is undocumented), W3 (issue #93's rationale),
W4 (`hook-spec.md:396`, `subsystem-file-structure.md:151` and `:190` still say
three), W5 (the agreement test cannot observe `add`), S1 (orphaned reap comment at
`hook_dispatch.rs:892`), S2, S3, S4.
