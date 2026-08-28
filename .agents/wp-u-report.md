# WP-U — the hook guard-path performance fixes

Hat 2 (optimization), so every claim below is a measurement and the "before" it is measured
against was taken in **this worktree** (`.agents/worktrees/wp5-u`, based on `31a9154`) before any
edit. Nothing in `hook_dispatch_latency.md`'s baseline was rewritten; the after-column was appended
there under [After WP-U](./hook_dispatch_latency.md#after-wp-u--what-the-fixes-moved).

| Finding | Outcome | Headline |
|---|---|---|
| **F-1** — 24-worker runtime per tool call | **Fixed** (`5a37b8b`) | ext4 warm no-match **3.40 → 1.44 ms** p50; **24 → 0 `clone3`**; cold **+4.5 ms**, a real and deliberate trade |
| **F-2** — `NoMatch` record per open | **Fixed** (`54443b9`) | 9P, ten armed hooks **139.21 → 10.44 ms** p50, p99 **316 → 22.7 ms**; marginal per hook **13.8 → ~0 ms** |
| **F-3** — `sh` resolved through `PATH` | **Recorded, not fixed** | Publisher-authored argv; reasoning below, plus a second site the baseline report did not name |

Gates: `cargo fmt`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo test --bin grim`
(2897 passed) and `task verify` (1071 acceptance tests, 1 xfailed) green before each commit;
`task bench:confirm` green after each change. `.claude/tests/uv.lock` and `test/uv.lock` reverted;
nothing staged with `git add -A`; `commit-verified` never hand-stamped (both commits waited for
`task verify` to stamp it).

- [Environment and how to reproduce](#environment-and-how-to-reproduce)
- [F-1 — the runtime flavor](#f-1--the-runtime-flavor)
- [F-2 — one append per invocation](#f-2--one-append-per-invocation)
- [F-3 — the PATH-resolved interpreter](#f-3--the-path-resolved-interpreter)
- [What I found wrong](#what-i-found-wrong)

## Environment and how to reproduce

Same host and harness as the baseline report: WSL2 (kernel `6.18.33.2-microsoft-standard-WSL2`),
Intel Core Ultra 9 285HX, **24 logical cores**, `grim 0.13.0` release, `hyperfine 1.20.0`,
`strace 6.19`, `/bin/sh` → dash.

```sh
task bench:hooks                                          # ext4 matrix
task bench:hooks BENCH_ROOT=/mnt/c/temp/grim-bench-wp5u    # the 9P row
task bench:syscalls                                       # strace -c attribution
task bench:confirm                                        # the credibility check
```

Every row came out of that harness unmodified — **no new bench row was needed**, so
`taskfiles/bench.taskfile.yml` is untouched. The two headline command lines it runs, verbatim:

```sh
hyperfine -N --warmup 20 --runs 500 \
  --input {{.BENCH_ROOT}}/payload_read.json \
  --command-name 'no-match/matcher×10 · runtime · warm' \
  '{{.GRIM}} hook run --client claude --event PreToolUse \
     --table {{.BENCH_ROOT}}/armed10/dispatch.json --root 0123456789abcdef0123456789abcdef'

hyperfine -N --warmup 0 --runs 150 \
  --input {{.BENCH_ROOT}}/payload_read.json \
  --prepare 'python3 {{.BENCH_ROOT}}/evict.py {{.GRIM}} \
     {{.BENCH_ROOT}}/armed1/dispatch.json {{.BENCH_ROOT}}/bin/grim-hook' \
  --command-name 'no-match/matcher×1 · chain · cold' \
  '/bin/sh {{.BENCH_ROOT}}/armed1/registration.sh'
```

`strace` is still not installed on this host; obtained unprivileged exactly as the baseline
report describes (`apt-get download strace libunwind8 && dpkg-deb -x …`, `LD_LIBRARY_PATH` at the
extracted `libunwind-ptrace.so.0`), then put on `PATH` for `task bench:syscalls`.

**One caveat on absolute values.** The same harness at the same commit reproduced the baseline's
9P `matcher×10` row within 2 % (139.21 vs 142.07) but its ext4 warm rows ~9 % high (3.40 vs 3.20
for `no-match/event`) — a different worktree, a busier machine. That is exactly why every delta
here is within-tree, and why the numbers should be read as deltas rather than as a re-issue of the
platform table.

## F-1 — the runtime flavor

### How the per-arm choice is made

`main.rs` did build the runtime before it knew the subcommand — but only because it chose to.
clap has **already answered** by then: `parse_cli` runs at what was line 149 and the runtime was
constructed at line 172. So the fix needs no argv peek and no second parse:

```rust
let runtime = match build_runtime(runtime_flavor(cli.command.as_ref())) { … };

fn runtime_flavor(command: Option<&Command>) -> RuntimeFlavor {
    match command {
        Some(Command::Hook(hook)) if matches!(hook.command, command::hook::HookCommand::Run(_)) =>
            RuntimeFlavor::CurrentThread,
        _ => RuntimeFlavor::MultiThread,
    }
}
```

I rejected the argv pre-scan deliberately. `color::mode_from_args` has to pre-scan because clap
renders `--help` *during* parse, so the color decision is needed before clap runs; the scheduler
decision is not, and a second hand-rolled parser deciding which scheduler `hook run` gets would be
a second spelling of clap's own answer — one that drifts the first time a flag moves.

**Nothing about the C-007/B1 ordering moved.** `runtime_flavor` reads the parsed command and
resolves nothing; `app::run` still returns the `Hook(Run)` arm before `Context::new`, and the
existing `app_dispatches_the_runtime_before_it_builds_a_context_b1` test still pins it.

`enable_all()` on both arms is load-bearing, not copied: the dispatch path needs the I/O driver
(stdin, the payload's pipes) and the time driver (the per-hook timeout).

### What pins it

Two tests, one behavioural and one structural — the second in the idiom
`src/command/hook.rs` already uses for C-007 and C-009:

| Test | Where | What it would catch |
|---|---|---|
| `the_hook_runtime_runs_on_a_single_worker_f1` | `src/main.rs` | Parses the real launcher argv, builds the runtime through the shipped function, asserts `runtime.metrics().num_workers() == 1`. A flavor change fails here even if the code still *looks* right. |
| `every_other_command_keeps_the_multi_thread_runtime_f1` | `src/main.rs` | Seven other argvs (incl. `hook list`, `tui`, `mcp`, bare `grim`) must stay `MultiThread` — the "do not regress any other command" half. |
| `the_hook_runtime_is_not_built_on_the_multi_thread_scheduler_f1` | `src/command/hook.rs` | Source-level: `main.rs` must contain `build_runtime(runtime_flavor(`, must contain `new_current_thread`, and must hold **exactly one** `tokio::runtime::Runtime::new()` — a second constructor is a path that bypasses the decision. |

### The numbers

ext4, release, 500 warm / 150 cold runs, p50 (F-1 alone, before F-2):

```
scenario · chain · thermal                p50    was    Δ
no-match/event · runtime · warm          1.41   3.40  -1.99
no-match/matcher×1 · runtime · warm      1.81   3.78  -1.97
no-match/matcher×1 · chain · warm        2.54   4.68  -2.14
match/1 · chain · warm                   3.06   5.04  -1.98
no-match/event · runtime · cold         13.99   8.71  +5.28
match/1 · chain · cold                  16.28  10.77  +5.51
```

`strace -c -f`, the cheapest guard path (`no-match/event`): **974 → 106 syscalls**, `clone3`
**24 → 0**, `openat` 15 → 11, `statx` 5 → 2. Zero `clone3` because a current-thread runtime starts
no workers at all and the blocking pool is only spun up when something is actually written — which
that path never does.

### The cold regression is real, and I did not talk myself out of it

The cold rows got **worse by 4.1–5.7 ms**, on every scenario, on both filesystems. My first
instinct was contamination (the team lead was deleting ~54 G of stale worktrees during that run),
so I re-ran on a quiet disk: it reproduced within 0.3 ms. So I attributed it instead, with four
binaries identical except the hook arm's scheduler, on ext4 (`target/bench/variants/`), `hyperfine
-N`, `no-match/event`, 300 warm / 100 cold runs, `--prepare` evicting all four:

| hook-arm scheduler | warm mean | cold mean (order A) | cold mean (order B, reversed) |
|---|---|---|---|
| `current_thread` | **2.0 ms** | 13.9 ms | 11.8 ms |
| `multi_thread`, 2 workers | **2.0 ms** | 14.0 ms | 13.5 ms |
| `multi_thread`, 6 workers | **2.0 ms** | **7.7 ms** | **7.6 ms** |
| `multi_thread`, 24 workers (the baseline) | 5.5 ms | 9.3 ms | 9.5 ms |

Reversed command order reproduces it, so it is not a hyperfine ordering artifact. Mechanism, stated
as the hypothesis it is: the worker pool was **incidentally parallelising the major faults** that
pull the 26.8 MB binary back off a WSL2 VHD, where per-I/O latency is high and queue depth is
everything. Warm needs no such help, which is why warm is flat from 1 to 6 workers and only the
24-thread spawn cost shows up there.

**I shipped `current_thread` anyway**, and the `worker_threads(6)` row is the one thing in this
report I would hand back to the owner as a live option:

- Warm is what a session pays. Cold happens once per idle period; warm happens on every tool call
  after it, and break-even is about three calls. An agent doing a hundred tool calls pays the cold
  penalty once and collects −2 ms ninety-nine times.
- `6` is a constant tuned to one 24-core WSL2 host's page-fault behaviour, on a machine where three
  of the six platform rows are still unmeasured. It would be a magic number defended by one host.
- The honest fix for a 5 ms cold row is the 26.8 MB binary, not the scheduler.

Flipping it is a one-line change in `build_runtime` if the owner disagrees; the doc comment on
`RuntimeFlavor` carries the same numbers so nobody has to rediscover them.

## F-2 — one append per invocation

**Shape chosen: hoist *and* batch — they turned out to be the same fix.** The report offered two
cheaper shapes; implementing "hoist `create_dir_all` + the open out of the per-record path" for a
set of records *is* "batch one invocation's records into a single append", because the hoisted
prelude has to be paid somewhere and the only honest place is once around the whole set. So
`AuditLog::append_all(&[AuditRecord])` pays `create_dir_all`, the rotation `statx` and the open
once, builds one buffer of capped JSONL lines, and does one `write_all`; `pipeline::record_no_matches`
hands it the declined set that `run::dispatch`'s matcher loop already collects.

**The forensic answer is untouched**, which is the point — `AuditOutcome::NoMatch`'s own doc
mandates the record and dropping it was never on the table. Same records, same fields, one line
each, same order, still written **before** anything is spawned (the batch is flushed before the
`matched.is_empty()` early return, so a tool call where every hook declined still records why).
`task bench:confirm` reports 1 and 10 records for the two decline scenarios exactly as before.

Two deliberate small consequences, both stated in the code:

- **Rotation slack.** The threshold is now checked once per batch, so the trail can overshoot
  `MAX_LOG_BYTES` by one batch (armed count × 4 KiB) instead of by one record. The stated bound is
  `2 * MAX_LOG_BYTES`; a batch is orders of magnitude inside that slack.
- **Tearing.** One `write_all` of N whole lines is still one positioned append; a batch only tears
  on a short write, which a regular file does not return outside `ENOSPC`/signal — and the cost of
  that was always one torn line, never the file.

**A post-spawn record is still written on its own.** `invoke` cannot batch a record with the next
hook's, because the next hook has not run yet; holding it back across a spawn would trade
durability for a syscall. So `append_record` survives as the single-record wrapper over the same
seam.

### G-6 — yes, it vanished for free

WP-K's G-6 (the writability probe duplicating `append`'s prelude) disappeared rather than being
fixed separately. `AuditLog::writable()` now *is* `append_all`'s prelude — `ensure_parent()` then
`open_append()`, the same two private helpers the append uses — and `pipeline::audit_is_writable`
shrank to the `spawn_blocking` wrapper. Two spellings of "how the trail is opened" became one, on
the type that owns it.

One thing did **not** come for free and is worth naming as a scope call: once every caller routed
through the batch, `AuditLog::append` was dead code (clippy's `dead_code` caught it under
`-D warnings`). I deleted it and moved its fail-closed caller contract onto `append_all` verbatim
rather than keeping a one-line forwarder alive to avoid touching structure. That is a deletion the
optimization itself caused, not a drive-by refactor — but it is the one structural edit in these
two commits, so it is flagged here rather than buried.

### The numbers

Warm p50, ten armed-but-unmatched hooks on one tool call (`matcher×10`), and the marginal cost per
armed hook, `(p50(×10) − p50(×1)) / 9`:

| | ext4 p50 | ext4 marginal | 9P p50 | 9P p99 | 9P marginal |
|---|---|---|---|---|---|
| baseline `31a9154` | 4.01 ms | +0.042 ms | 139.21 ms | 316.44 ms | **+13.8 ms** |
| after F-1 only | 2.19 ms | +0.042 ms | — | — | — |
| after F-1 + F-2 | **1.95 ms** | **−0.001 ms** | **10.44 ms** | **22.69 ms** | **−0.13 ms** |

A negative marginal is noise around zero, which is the honest reading: **the linear term is gone**
on both filesystems. Ten armed hooks now cost what one costs.

`strace -c -f`, `matcher×10`, stable across two runs: total **1212 → 249**, `mkdir` **10 → 1**
(the `EEXIST` storm is gone), `openat` **25 → 12**, `statx` **25 → 4**, `write` 11 → 7. The ×10
path's file-touching columns are now identical to the ×1 path's.

The other 9P rows move too — every p99 on that filesystem collapses (`matcher×1` runtime warm
175.54 → 44.44, chain warm 156.71 → 52.34) because the per-record opens were the tail. The ±3 ms
p50 wobble on 9P `match/1` is noise on a row whose baseline p99 was 153 ms; the 9P warning in the
baseline report (cold and warm are one population there) still applies to everything on that
filesystem except the `matcher×10` row, which moved by 128 ms.

## F-3 — the PATH-resolved interpreter

**Decision: it stays, and the published handler-argv contract is unchanged.** But the baseline
report's framing is incomplete, and the correction matters more than the decision.

### It is two sites, not one, and only one of them is grim's

| Site | Who names `sh` | Measured cost |
|---|---|---|
| `HookHandler::Argv(["sh", "guard.sh"])` | **the publisher**, in `hook.toml` — the bench fixture's form, and the documented *preferred* form | 23 failing `execve` per spawn, ~0.1 ms of 0.57 ms on ext4 |
| `HookHandler::Command("guard.sh")` → `pipeline::handler_command` runs `sh -c <line>` (`cmd /C` on Windows) | **grim itself**, in `src/command/hook/pipeline.rs:874` | same class; not exercised by any bench row |

The report named only the first. The second is the interesting one, because there the choice is
grim's own and no contract stops grim from spelling it `/bin/sh`.

### Why it still stays

- **The argv case is data, not code.** Rewriting `argv[0]` means grim silently executing a
  different binary than the publisher named, on a path whose whole point is that the user approved
  a pinned digest of exactly what was named. That is a widening of grim's role at the exec boundary
  and it is not a performance decision.
- **Portability cuts the wrong way.** `/bin/sh` is not POSIX-guaranteed (the standard shell is
  `getconf PATH`-relative), and three of the six platform rows in the baseline table are still
  unmeasured — including the one where `hook_launcher.rs` marks its own registration string
  runtime-unverified. Hard-coding an absolute interpreter is a behaviour change that would first be
  *observed* on the platforms nobody has run yet.
- **The security argument is weaker than the W9 comparison suggests, and I checked rather than
  assumed.** W9's absolute launcher path is CWE-426 mitigation for a path *grim bakes and executes*
  — grim owns it. The handler argv is executed with the client's inherited environment (grim adds
  the `GRIM_HOOK_*` allowlist via `command.env`, and deliberately does **not** `env_clear`), so a
  hostile `$PATH` reaches the payload either way: the payload is a script that resolves `grep`,
  `git`, everything it calls through that same `$PATH`. Pinning `argv[0]` closes the first hop of a
  chain whose remaining hops stay open, which buys the appearance of a control rather than a
  control.
- **The win is ~0.1 ms of a 0.57 ms spawn**, on the *matched* path only — the guard path every
  tool call pays does not spawn at all.

### What I would hand to the contract owner

If the owner wants this closed, the effective fix is **not** a grim-side argv rewrite. It is one of:
(a) document that publishers should name an absolute interpreter, and let `grim build` warn on a
bare `sh`; or (b) control the child's `PATH` in `envelope::environment`, which closes every hop
rather than the first — a real change to the payload environment contract, out of this package's
file set and worth its own decision. The `HookHandler::Command` site could be pinned to `/bin/sh`
unilaterally today, but doing only that would leave the majority form (`argv`) unpinned while
implying the question was settled, and it would ship a behaviour change with **no bench row
measuring it**, which is precisely what Hat 2 forbids.

## What I found wrong

Beyond the three findings:

1. **The cold/warm trade in F-1 is a measured negative result**, not a rounding error, and the
   baseline report did not predict it. It is reported in full above and in the appended section of
   `hook_dispatch_latency.md` rather than left in a commit message.
2. **`no-match/matcher×1` was never going to improve from F-2 and did not** — one record is one
   open either way. Its ext4 warm row moved +0.15 ms between the F-1 and F-2 measurements, which is
   run-to-run drift, not a regression. Saying so is cheaper than implying the fix helped everywhere.
3. **This worktree's git submodules were uninitialized** (`external/docker_credential`,
   `external/rust-oci-client`), so `cargo build` failed outright until `git submodule update
   --init`. Anything that creates agent worktrees should init them.
4. **`/mnt/wsl/share` hit 100 % (1.4 MB free of 192 G)** mid-package and `rustc` died with
   `IO failure on output stream: No space left on device`. ~54 G was `target/` in merged worktrees.
   I did not touch other agents' trees; I moved my own `target/debug` to `/home/mherwig/.cache/wp5u-debug`
   and left a symlink, keeping `target/release` in-tree so before/after binaries stayed on one
   filesystem. **That symlink is still in place** — remove it and let cargo recreate the directory
   when the branch lands.
5. **A trap for the next person benchmarking this**: my first attribution run put the four variant
   binaries in the session scratchpad, which is **tmpfs**, where `POSIX_FADV_DONTNEED` silently does
   nothing. Every "cold" row came out equal to its warm row — the exact failure mode F-4 recorded
   for `--prepare 'sync'`, wearing different clothes. The numbers above were re-taken with the
   variants on ext4.
6. **Pre-existing, not touched:** `install::hook_dispatch::read_table` does blocking `std::fs` I/O
   inside the async dispatch path (`quality-rust.md` calls that Block-tier). It was there before
   this package and is harmless on a current-thread runtime with nothing else scheduled — but F-1
   removed the worker pool that used to make it *look* harmless, so it is worth someone's decision
   rather than a silent inheritance.
