# Round 1 — performance review (rv-perf)

Verdict: **1 Block, 7 Warn, 5 Suggest.** All measured; unmeasurable claims are marked.
Env: WSL2 6.18.33.2, 24 cores, `hyperfine 1.20.0 -N`, `strace -c -f`, ext4 `$GRIM_HOME` unless noted.
Line numbers as of `a5399fd`.

**Baseline reproduced before measuring:** `task bench:hooks` gives no-match 1.38 / matcher×1 1.89 /
matcher×10 1.92 ms p50 warm, within noise of `wp-u-report.md`'s after-column. The harness agrees with
itself, so the deltas below are trustworthy.

## Block

**B-1 — `src/install/hook_dispatch.rs:852-871,874`: the dispatch table grows without bound, and at
1 MiB every hook on the machine silently disarms.**

`converge_root` inserts or removes **only** the current token's entry; no cross-root sweep exists
(grepped `hook_registrar.rs`, `prune.rs`, `status.rs`). A root whose workspace was deleted, renamed or
moved is never visited again — the token is an HMAC of the path, so a move mints a new token — and its
rows persist forever. Nothing bounds the number of roots, and the guard path's cost is linear in the
**whole table**, not the armed set.

Executed: a 1,064,542-byte table (1,680 rows) trips `MAX_TABLE_BYTES`:
```
WARN … dispatch table … was not usable (Oversize); no hook ran
exit=0
```
Every hook on the machine stops firing, exit 0 by design (I3), one `warn` line no client surfaces.
Marginal 624 B/row pretty-printed ⇒ the cap arrives at ~1,680 rows: 168 stranded roots × 10 entries.

Filed Block because it is unbounded growth whose terminal state is a **silent failure of the whole
feature**. Remediation: reap roots during convergence whose recorded `root` path no longer exists (the
entry already carries it, `:856`), or stamp a last-seen and expire.

## Warn

- **W-1** `pipeline.rs:457` + `audit.rs:499,503,504` — **F-2's fix never reached the matched path.**
  `record_no_matches` batches; `invoke` does not. 9P, trail-only delta: matched 1 hook 35.7 ms → 10 hooks
  **246.1 ms**, marginal **+23.4 ms/hook**; the batched decline control is flat (28.3 → 29.2). `strace`
  over 10 matched observers: **11 `mkdir`**, 21 trail touches, vs 1 `mkdir` on the decline path. Same
  pathology F-2 was credited with removing, at +23.4 rather than +13.8. Hoist `ensure_parent()` and the
  rotation check to once per invocation; each record stays one positioned `write_all`, so durability is
  untouched.
- **W-2** `pipeline.rs:323` → `audit.rs:536-537` — the C-012 writability probe duplicates the append
  prelude. `strace` on match/1: exactly **2 × `mkdir(EEXIST)`** and **2 × open** of the trail. G-6 merged
  the two spellings; the two calls remain. Have `writable()` return the open handle.
- **W-3** `hook_dispatch.rs:723,735` — `read_table` parses the whole table **twice**, and the guard path
  is linear in total table size. `unknown-root` (pure reader cost): 1 row 1.5 ms → 1,650 rows /
  1,045,531 B **5.7 ms (3.8×)**; 9P 6.0 → 15.5 ms. Parse attribution at the cap: `Value` + `from_value`
  1.92 ms; schema-probe struct + one typed `from_slice` 1.22 ms; single typed `from_slice` 0.92 ms.
  Replace the untyped `Value` stage with a `SchemaProbe` struct — preserves W2's
  `UnknownSchema`-vs-`Unparsable` asymmetry exactly. Keep the per-row re-check; that is deliberate.
- **W-4** `pipeline.rs:371` — **matched latency is the sum, and an observer that cannot contribute to
  the answer still blocks the tool call.** Phase 2 awaits `gatekeepers.chain(&observers)` one at a time:
  with a `sleep 0.3` payload, 1 observer 308.8 ms → 3 observers **919.9 ms (+611 ms)**. `assemble`
  filters observers out of every field; an observer's only effect is its audit record. Decision O
  mandates serialization for **mutators** and mutators-before-gatekeepers only. Run phase 2 concurrently,
  collecting in declaration order: N×t → max(t).
- **W-5** `installer.rs:415` + `command/install.rs:448` — `grim install` reads and parses the whole
  table **twice** in a project declaring no hook. The fast-path probe tests `dispatch_path().is_file()`,
  which **any other workspace's** hooks satisfy. Hook-free project: no table 5.4 ms → 1 MiB table
  **14.4 ms (+9.0 ms)**; `strace` shows 2 opens plus the lock, attributed by `addr2line` to
  `converge_clients` and `armed_after_convergence`. Pass the table `converge_clients` already read.
- **W-6 (deferred)** `oci/hook.rs:435` + `pipeline.rs:838` — no aggregate timeout budget and `timeout`
  is an uncapped publisher `u64`. Worst case one tool call waits Σ of declared timeouts. Under **N5** a
  slow hook the user installed is a non-goal, so not a defect claim — but a `timeout = 600` typo costs a
  ten-minute stall with no backstop. A cap changes what a published `timeout` means: owner's call.
- **W-7** = the glob-compile finding below.

## The two peer findings, measured

**Glob compile per invocation (`run.rs:455-465`).** `MATCHER_ALLOWED` excludes `{}`, `[]`, `,` and
bounded repetition — **exactly the constructs behind the regex crate's ~44 ms worst case** — so C-007's
cited catastrophe is unreachable. But the cost is publisher-controllable and linear:

| armed hooks, all declining | mean |
|---|---|
| 1 × exact `Bash` (no compile) | 2.1 ms |
| 1 × benign glob | 2.2 ms |
| 1 × worst legal `"*"×256` | **2.9 ms** |
| 10 × exact | 2.2 ms |
| 10 × benign glob | 2.5 ms |
| 10 × worst legal | **10.1 ms — 4.6×, +7.9 ms** |

~30 µs per benign glob, ~0.79 ms per worst-legal glob, per armed hook, per tool call. 32 wildcarding
hooks ≈ 25 ms of pure compile on every tool call. **Warn, not Block** — the cited figure is unreachable
and the realistic ceiling is ~10 ms — but it violates the *letter* of C-006 ("stored precompiled") and
C-007. Preferred remedy: (a) compile once per **distinct** matcher string per invocation (fixes the
common N-hooks-same-matcher shape: 10.1 → ~2.9 ms) **plus** (c) amend C-006/C-007 to state the measured
bound. Option (b), a wildcard-count cap at build, would refuse a matcher a publisher could already have
shipped — a freeze consideration.

**`record_no_matches` — my reading differs from the quoted figures.** +0.04 ms ext4 / +14.1 ms per hook
on 9P is the **pre-WP-U baseline**; F-2 removed that linear term and it is gone (9P: 1 declining hook
28.3 ms → 10 hooks **29.2 ms, flat**). What remains is a **fixed per-invocation** cost: 164 syscalls vs
106 on the early-return path, **+0.51 ms** end to end. Isolated 9P syscall costs: `create_dir_all` on an
existing dir 1.60 ms, rotation `stat` 1.10 ms, open+write 9.69 ms — ~2.7 ms of each ~14.3 ms append is
prelude. The end-to-end 9P A/B (18.7 → 17.5) is inside that filesystem's noise, so the end-to-end win is
**not** claimed; only the isolated syscall costs.

**`grim status`'s `declares_a_hook` guard — verified, costs nothing.** Hook-free project with a
1,045,531-byte table present: `strace` shows **zero** touches of any hook path. 5.5 vs 5.2 ms (noise).
`grim install` has no equivalent gate and does pay — W-5.

## Suggest
- **S-1** `pipeline.rs:476,448` — `spawn_blocking` for a ~30 µs append on a one-task runtime. Isolated
  cold-pool: inline 0.028/0.144 ms p50/p99; `spawn_blocking` 0.103/**6.01** ms. End-to-end p99 delta is
  only +0.66 ms, so the 6 ms p99 is **not** reproduced end to end and is not asserted. Drop the wrapper
  for `record_no_matches` (runs before anything is spawned); keep it in `invoke`.
- **S-2** `taskfiles/bench.taskfile.yml:35,390` — the harness measures `profile.release`; the **shipped**
  binary is `profile.dist`, and `:390` actively rejects it. Measured cold: release 27,345,064 B →
  12.7 ms; **dist 22,170,864 B → 9.8 ms (1.29× faster)**; warm identical. This **strengthens** F-1: on
  the artifact users run, the cold penalty is materially smaller than published.
- **S-3** `envelope.rs:218` — the per-hook envelope re-validates an invariant `raw`: +0.58 ms/hook at
  142 B, **+5.1 ms at 4 MB**, of which the redundant shallow parse is 0.42 ms (~10% avoidable). Validate
  once in `compose`. Deferred.
- **S-4 (not a finding — recorded so nobody "fixes" it into a regression)** blocking `std::fs` in the
  async dispatch path is correct here: the current-thread runtime has one task, so there is no reactor to
  starve, and moving it to `spawn_blocking` would **cost** ~0.08 ms. Leave it, with a comment.
- **S-5** No `MutexGuard`-across-`await` and no unbounded reads anywhere on the hook path; caps verified
  present (payload 8 MiB+1 so over-cap is detectable, child stdout 64 KiB, table 1 MiB). Clean.

## Verified — the F-1 cold trade is sound, corroborated by a measurement the report did not make
`grim --version` short-circuits inside clap and builds **no runtime at all**: cold **11.6 ms**, against
the current-thread cold row of 12.0 ms — **+0.4 ms over a floor with no runtime in it.** So
`wp-u-report.md`'s attribution is **verified**: 11.6 of 12.0 ms is pre-runtime binary faulting, and "the
honest fix for the cold row is the binary's size, not the scheduler" holds — with S-2 showing that lever
moving it (−2.9 ms for −5.2 MB). The 24-worker baseline's 9.3 ms cold row sits *below* the no-runtime
floor, explicable only by the pool parallelizing major faults — the report's own hypothesis, now
corroborated. Break-even ≈ 2.2 calls. **Ship `current_thread`.**

Residual risk, **unverified**: "once per idle period" assumes the binary stays resident; under sustained
page-cache pressure every call could be cold and the trade inverts. Not simulated, not claimed.
