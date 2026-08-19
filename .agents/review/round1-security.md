# Round 1 — security review (rv-security)

Verdict: **1 Block, 3 Warn, 2 Suggest.** Binary under test: release build against `a5399fd`.
Answer to "do P-1..P-7 hold as a class": **P-2 does not — that is the Block.** P-1, P-3, P-4, P-6, P-7
hold. P-5 holds for the half it was written about and misses the rest of its class (W-2).
**No other path into the dispatch table was found.**

## Block

**B-1 — a hook binding name from a repository file escapes `$GRIM_HOME` and overwrites arbitrary
files. P-2's remediation is defeated by one token.** T3 escalating with T1; invariant **I1**.

`src/install/installer.rs:456` gates on `is_reserved_binding_name(&artifact.name)` — **exact string
equality** against `["bin","dispatch.json","payload"]` (`src/oci/hook.rs:179`).
`src/install/target.rs:283-289` then joins that same string onto `$GRIM_HOME` via `payload_dir(...)`
with **no containment check**, and anchor classification runs *after* materialization. `[hooks]` table
keys never pass through `SkillName::parse` — that call exists only in `command/add.rs` (typed names)
and `resolve/resolver.rs` (bundle members), neither of which a committed `grimoire.toml` reaches.

Executed. Preconditions: the victim's **global** config carries one `[[registries]]` entry with
`trust_hooks = true` (the documented trust act); the cloned repo's committed `grimoire.toml` supplies
both the feature flag and the traversal name. No prompt, no `--allow-hooks`.

```
[hooks] "../../../../victimdir" = "localhost:5000/probe-ow/shell-guard:1"
install rc=1  err: cannot classify install target '…/hooks/payload/7ff5c0…/../../../../victimdir'
VICTIM guard.sh now: '#!/bin/sh\n# ATTACKER CONTENT\ntouch /tmp/OWNED\n'
```
A second run with `"../../../hooks/bin"` wrote `hook.toml` into `$GRIM_HOME/hooks/bin/`, past the
reserved-name gate. Does **not** reproduce: the launcher shim survives (grim regenerates it in the same
command) and no state record is written, so P-2's uninstall-reap escalation does not land. Exits 1 with
a message — but the write precedes the refusal and other artifacts still install, so it reads as a typo.

Remediation: `SkillName::parse(&artifact.name)` at `installer.rs:456` beside the existing gate and
before materialization; mirror in `hook_registrar::desired_entries`.

⛔ Root cause is **pre-existing and wider**: the same traversal via a `[skills]` key needs *no* trust
gate at all, demonstrated in the same session (`victimdir/SKILL.md` overwritten, rc=1, same message),
and `main` calls `SkillName::parse` in the same two places. **File the shared declaration-key defect
separately — it is out of this diff.** The hook instance is in-diff and is the one where the escaped
tree is the *armable* tree.

## Warn

**W-1 — the timeout does not bound the payload at all** (peer finding, judged **worse than filed**, and
**in scope, not N5**). T1; I3 **and I5**.
```
declared hook timeout 2s; payload: printf '{}' ; exec 1>&- ; sleep 60
STILL RUNNING after 25.0s
```
`read_to_end` returns EOF inside the timeout, so the `Ok` branch runs `child.wait().await` unbounded;
`kill_on_drop` cannot help because nothing is dropped. `pipeline.rs:846-848` says *"Grim enforces the
timeout, not the vendor"* — a control described as prevention that is not one. Stays Warn only because
all three vendors emit `registration.timeout` into their config as a backstop. Remediation: bound the
whole of (write stdin → read stdout → wait), and **cap `entry.timeout` at a grim-owned maximum** so a
publisher cannot declare 600s.

**W-2 — `sanitize` closes Unicode `Cc` only; `Cf` reaches the projected verdict, the audit trail and the
environment. P-5's remediation covers half its class.** T1 (projector) / T4 (trail); I5.
`src/hook/audit.rs:408-419` filters `char::is_control` (Cc). This repo's **own** `src/hook/trust.rs`
uses `escape_debug` for the trust prompt and says why: *"covers what `char::is_control` misses
(U+202E)"*. Executed on both sinks: U+202E and U+2066 survived into
`permissionDecisionReason`/`additionalContext`, and the trail line carries the **raw UTF-8 override**
so `cat hook_audit.jsonl` renders reordered. The ESC/CR/LF half *is* closed — no overclaim.
Remediation: extend to `Cf` (or use `escape_debug`, the spelling `trust.rs` already chose); state at
`is_flat_scalar` whether it is deliberately narrower.

**W-3 — the two files carrying invocation data are created at the umask, and the `0o700` containment
does not self-heal.** T5; I6 + a Principle 9 self-heal miss.
`src/hook/audit.rs:541-553` and `src/command/hook/pipeline.rs:934-975` write with no `.mode()`, while
their siblings `dispatch.json` and `root-key` are explicit. `ensure_hooks_dir` sets `0o700` only
`if !existed`. Executed: loosening `hooks/` to `0o755` survives a re-install and a `hook run`;
`hook_audit.jsonl` is `0o644`; with `payload = "file"` the envelope file is `0o644` and carries the
**verbatim tool input** (`"command":"deploy --token ghp_SUPERSECRET"`). Remediation: `.mode(0o600)` on
both writes; make `ensure_hooks_dir` re-tighten unconditionally.

## Suggest

**S-1** `src/oci/hook.rs:111,576` assert the payload-file path is *"now hash-derived"*. It is
`payload-<pid>-<slot>.json`, and `write_payload_file`'s own doc says **"Deliberately not a hash"**
because C-009's source-level ban makes one impossible there. Fix the two lines.

**S-2** `catalog/hooks/tool-call-logger/log.sh:21` appends to a predictable `/tmp` path. T5. First-party,
so it is the pattern readers copy; neither the README nor the description mentions the shared-machine
caveat. Deferred — example hardening.

## On the peer's consent finding
`hook_consent.rs:157-167` — **agreed, real.** The behaviour (a grant that could not be recorded must not
arm) is the right fail-safe; only the *reported reason* is wrong. Graded Suggest rather than Warn since
no gate moves, but genuinely I5-adjacent.

## Checked and found sound (evidence in the reviewer's message)
P-1 as a class (one `hook_registration` call site on the convergence path; `union_of` consumes
`ArmedClient::rows`; `register_desired` pushes to both or neither); P-2's gate fires before any fetch or
write; P-3/P-6 at both seams (`validate` and `validate_installed` share `validate_entries`);
P-4/P-7 fixed; **10 dispatch refusals all exit 0 with nothing spawned**, against one positive control;
the runtime resolves no scope and never reads `$GRIM_HOME`; **a non-zero exit can never reach a
fail-closed client** (all three clients get `HookCommand::Shell` ending `*) exit 0`); bundle-delivered
trust keys on the member's own `LockedSource` (executed, with a paired positive control); the payload
location derives from `$GRIM_HOME`, never the record; trust cannot be granted from a repository or the
environment; the registered one-liner cannot be broken out of.

N-class dismissed as non-goals, not defects: hand-edited state/table/config (N1/N2); a same-privilege
process loosening `hooks/` (N2 — W-3 is filed against grim's failure to *re-tighten*); `--allow-hooks`
in one's own CI (N4); a hostile repo's own committed registration (N3, forged-root half closed and
pinned); a hook that is slow because the user chose it (N5 — distinct from W-1).

## Not reached — not covered by silence
`src/install/json_splice.rs` (+1376), `src/install/prune.rs` beyond P-2's residual,
`src/command/hook/list.rs` and the `status.rs` merge (verified structurally, not executed).
Probe scripts re-runnable under the session scratchpad.
