# Hook dispatch latency — WP-K's `Measure:` deliverable

**No pass/fail gate depends on any number here**, and that is the plan's explicit choice: a
latency gate on a path whose cost is dominated by process creation would fail on a loaded CI
runner and teach everyone to re-run it. The only value this table has is that a reader can trust
and reproduce it, so the methodology is stated at the same length as the results.

Harness: `taskfiles/bench.taskfile.yml` (`task bench:hooks`, `task bench:confirm`,
`task bench:syscalls`). Every number below came out of that harness; nothing was typed by hand.

- [Environment](#environment)
- [The platform table](#the-platform-table)
- [Full matrix — WSL2](#full-matrix--wsl2)
- [What each row cost, and why](#what-each-row-cost-and-why)
- [Methodology](#methodology)
- [How each scenario was confirmed](#how-each-scenario-was-confirmed)
- [Syscall attribution](#syscall-attribution)
- [Findings](#findings)
- [After WP-U — what the fixes moved](#after-wp-u--what-the-fixes-moved)

## Environment

| | |
|---|---|
| Platform | **WSL2** (Ubuntu 26.04 LTS on Windows) |
| Kernel | `6.18.33.2-microsoft-standard-WSL2` |
| CPU | Intel Core Ultra 9 285HX, **24 logical cores** |
| Memory | 62 GiB |
| Binary | `grim 0.13.0`, **`--release`** (`cargo build --release --locked`, rustc 1.95.0), 26,800,144 bytes |
| Shell for the chain rows | `/bin/sh` → **dash** |
| `hyperfine` | 1.20.0 |
| `strace` | 6.19 |
| `python3` | 3.14.4 (fixture generation, percentiles, page-cache eviction) |

**The binary profile is `release`, and the harness asserts it** — `report` prints
`NOT RELEASE — the number below is meaningless` if `GRIM` does not resolve inside a `release`
directory. A debug-profile number would be worthless here and must not be mistakable for this one.

## The platform table

The plan asks for one row per platform, with **WSL2 as a distinct row**. WSL2 needs *three* rows,
because its filesystems differ by more than an order of magnitude and folding them together would
report a number nobody experiences. Headline figure: the guard path a client pays on **every tool
call**, warm, at the full registration chain (so the B8 `fork` is inside it), over a
one-armed-hook table.

| Platform | Filesystem of `$GRIM_HOME` | no-match p50 | no-match p99 | match p50 | match p99 |
|---|---|---|---|---|---|
| **WSL2** — native process spawn | ext4 on a VHD block device (`/dev/sdd`) | **4.31 ms** | 5.54 ms | **4.88 ms** | 6.66 ms |
| **WSL2** — workspace on `/mnt/c` | ext4 (`$GRIM_HOME`), CWD on 9P | **3.90 ms** ¹ | 6.59 ms ¹ | not measured ² | not measured ² |
| **WSL2** — `$GRIM_HOME` on `/mnt/c` | **9p** (drvfs, `cache=0x5`, `msize=65536`) | **22.84 ms** | 170.97 ms | 25.85 ms | 52.35 ms |
| Linux (native, not WSL) | — | **not measured** | **not measured** | **not measured** | **not measured** |
| macOS | — | **not measured** | **not measured** | **not measured** | **not measured** |
| Windows (native, PowerShell registration) | — | **not measured** | **not measured** | **not measured** | **not measured** |

¹ Runtime-only (`grim hook run` directly), not the full chain — see note ².
² **Not measured, deliberately.** The three unmeasured platforms have no host available in this
session. **No number for them is inferred from the WSL2 rows**, because the two costs that dominate
every row here — process creation and per-`stat()` filesystem latency — are precisely the two that
differ most across those platforms; a Linux figure extrapolated from WSL2 would be a guess wearing
a measurement's formatting. Windows additionally runs a *different registration string* (the
PowerShell form in `hook_launcher.rs`, which `src/install/hook_launcher.rs` itself marks
**runtime-unverified** — no Windows host has ever executed it), so a Windows row is not merely
unmeasured, it would be measuring a code path nobody has yet run. The `workspace on /mnt/c` row was
measured at the runtime level only, as an ad-hoc control for the row below it rather than as a
full matrix.

Running the missing rows is one command on the relevant host: `task bench:hooks`. The report
stamps its own platform and filesystem, so a pasted result is self-identifying.

## Full matrix — WSL2

`scenario · chain · thermal`. `runtime` = `grim hook run` invoked directly; `chain` = the real
registered `sh` string → the launcher shim → grim, which is what a client runs and what carries the
B8 `fork`. p50/p99 are **nearest-rank** over the individual run times (no interpolation, so every
figure is a measurement rather than an average of two).

### `$GRIM_HOME` on ext4 — the native WSL2 row

```
scenario · chain · thermal                  runs   p50 ms   p99 ms   min ms   max ms
-------------------------------------------------------------------------------------
unknown-root · runtime · warm                500     3.17     3.86     2.71     4.32
unknown-root · runtime · cold                150     7.71     9.68     6.17    10.40
no-match/event · runtime · warm              500     3.20     4.01     2.71     5.02
no-match/event · runtime · cold              150     7.56     9.61     5.85    11.97
no-match/matcher×1 · runtime · warm          500     3.56     4.25     3.08     6.76
no-match/matcher×1 · runtime · cold          150     7.83    10.11     6.11    10.46
no-match/matcher×1 · chain · warm            500     4.31     5.54     3.77     6.08
no-match/matcher×1 · chain · cold            150     8.93    10.74     7.75    10.84
no-match/matcher×10 · runtime · warm         500     3.95     6.20     3.46     7.84
no-match/matcher×10 · runtime · cold         150     8.44    11.08     7.14    12.23
match/1 · runtime · warm                     500     4.13     4.91     3.59     5.16
match/1 · runtime · cold                     150     8.63    10.99     6.98    11.75
match/1 · chain · warm                       500     4.88     6.66     4.22     9.47
match/1 · chain · cold                       150     9.78    12.10     8.10    15.57
```

### `$GRIM_HOME` on `/mnt/c` — the 9P row

```
scenario · chain · thermal                  runs   p50 ms   p99 ms   min ms   max ms
-------------------------------------------------------------------------------------
unknown-root · runtime · warm                500     6.01    23.92     4.69    55.74
unknown-root · runtime · cold                150    10.09    15.87     8.23    27.11
no-match/event · runtime · warm              500     5.81    20.36     4.91    34.16
no-match/event · runtime · cold              150     9.98    31.34     8.11    81.40
no-match/matcher×1 · runtime · warm          500    15.29    33.02     8.98   176.52
no-match/matcher×1 · runtime · cold          150    14.44    49.47    12.46    93.90
no-match/matcher×1 · chain · warm            500    22.84   170.97    13.84   202.67
no-match/matcher×1 · chain · cold            150    25.72    76.49    17.29    99.65
no-match/matcher×10 · runtime · warm         500   142.07   295.51    79.88   351.54
no-match/matcher×10 · runtime · cold         150   181.34   318.75   106.20   339.92
match/1 · runtime · warm                     500    21.18    51.15    12.45   185.41
match/1 · runtime · cold                     150    18.28    36.32    15.70    52.54
match/1 · chain · warm                       500    25.85    52.35    15.91   139.80
match/1 · chain · cold                       150    29.41    68.76    20.40   105.01
```

> [!warning] The 9P cold rows are not a resolved cold/warm distinction
> On several 9P rows `cold` is *faster* than `warm` (`no-match/matcher×1`: 14.44 vs 15.29 ms).
> That is not a measurement of temperature. `POSIX_FADV_DONTNEED` does not reliably evict on
> `v9fs`, and 9P's own run-to-run variance (see the `max` column: 176 ms against a 15 ms p50)
> exceeds the effect being looked for. **Read the 9P rows as one population, not as two.** The
> ext4 rows, where eviction demonstrably works, are where cold and warm are separable.

### Workspace on 9P, `$GRIM_HOME` on ext4 — the control that matters

Measured directly, CWD on `/mnt/c/temp/grim-cwd` (`v9fs`), dispatch table and audit trail on ext4:

```
no-match/matcher×1  · runtime · warm   runs=500  p50=3.90  p99=6.59  min=2.98  max=11.90
no-match/matcher×10 · runtime · warm   runs=500  p50=4.35  p99=5.73  min=3.56  max= 6.81
```

Identical to the pure-ext4 rows (3.56 / 3.95 ms) within noise. **This is invariant I1 paying off as
a performance property, not only a security one:** because nothing armable lives in the repository,
a workspace on a 9P mount costs the dispatch path essentially nothing — the runtime never reads the
workspace, only `--table`. The expensive case is a 9P **`$GRIM_HOME`**, which is what a
Windows-side home directory produces.

## What each row cost, and why

All deltas from the ext4 warm rows, which are the ones with the least noise.

| Cost | Measured | Notes |
|---|---|---|
| Bare process startup on this machine | 0.41 ms p50 | `/bin/true`, 500 runs |
| grim's ELF load + dynamic linking + clap parse | 1.27 ms p50 | `grim --version`, 500 runs — clap short-circuits **before** the Tokio runtime is built, so this excludes it |
| **+ Tokio multi-threaded runtime + table read** | **→ 3.20 ms p50** | `no-match/event`, the cheapest path that reaches `run`. The ~1.9 ms step up from `--version` is overwhelmingly runtime construction — see [attribution](#syscall-attribution) |
| One `NoMatch` audit append (G-5), per armed hook | **+0.04 ms** on ext4 | (3.95 − 3.56) / 9 extra hooks |
| One `NoMatch` audit append (G-5), per armed hook | **+14.1 ms** on 9P | (142.07 − 15.29) / 9 extra hooks |
| Matcher decline vs. early return | +0.36 ms | 3.56 vs 3.20 — reads stdin, parses the payload, opens the audit trail |
| Match-and-dispatch vs. matcher decline | +0.57 ms | 4.13 vs 3.56 — the payload spawn, including a PATH search for `sh` |
| **The launcher chain, incl. the B8 `fork`** | **+0.75 ms** | 4.31 vs 3.56 (no-match); +0.75 ms on the match path too (4.88 vs 4.13) |
| Cold page cache (26.8 MB binary re-fault) | +4.3 to +5.1 ms | consistent across every ext4 scenario |

### The `fork` the guard path pays, stated in the no-match row

WP-P0's B8 dropped `exec` from the registration in favour of a spawn, so the registered string runs
the launcher as a **child** in order to distinguish "the launcher never ran" from "the launcher
returned a verdict" — which is what keeps a missing interpreter (127) or an `ENOEXEC` (126) from
reaching a fail-closed client as a denial. That `fork` is on the **guard** path, so it is paid on
every tool call **even when nothing matches**.

Its cost is the `chain` − `runtime` delta: **+0.75 ms warm, +1.10 ms cold, on ext4**; **+7.55 ms
warm on 9P**. So the honest reading of "no match is not free" on the native row is **4.31 ms p50 /
5.54 ms p99**, not the 3.56 ms the runtime alone reports. `strace -f` sees the fork as exactly one
`vfork` (dash implements the fork that way) plus two extra `execve` — the registration's `sh` and
the shim's `sh`; grim itself is reached by the shim's `exec`, so it adds no third process — and
+12 `stat`-family calls, which are the two `[ -f "$L" ] && [ -x "$L" ]` guard pairs plus each
shell's own startup.

## Methodology

### Exact command lines

Every measurement is a `hyperfine` invocation from `taskfiles/bench.taskfile.yml`. The two that
produce the headline figures, verbatim (`{{.GRIM}}` = the release binary, `{{.BENCH_ROOT}}` =
`target/bench/hooks` for the ext4 row and `/mnt/c/temp/grim-bench` for the 9P row):

```sh
# no-match, runtime only, warm
hyperfine -N --warmup 20 --runs 500 \
  --input {{.BENCH_ROOT}}/payload_read.json \
  --command-name 'no-match/matcher×1 · runtime · warm' \
  '{{.GRIM}} hook run --client claude --event PreToolUse \
     --table {{.BENCH_ROOT}}/armed1/dispatch.json --root 0123456789abcdef0123456789abcdef'

# no-match, full registration chain (includes the B8 fork), cold
hyperfine -N --warmup 0 --runs 150 \
  --input {{.BENCH_ROOT}}/payload_read.json \
  --prepare 'python3 {{.BENCH_ROOT}}/evict.py {{.GRIM}} \
     {{.BENCH_ROOT}}/armed1/dispatch.json {{.BENCH_ROOT}}/bin/grim-hook' \
  --command-name 'no-match/matcher×1 · chain · cold' \
  '/bin/sh {{.BENCH_ROOT}}/armed1/registration.sh'
```

The remaining twelve differ only in `--event`, `--table`, `--root`, `--input`, and whether the
command is the binary or the registration script. Read the taskfile for the full set rather than
trusting a paraphrase.

### Why each flag

- **`-N` (`--shell=none`)** — mandatory in a sub-5 ms regime. A shell spawn on this machine costs
  the same order as the thing being measured, so leaving hyperfine's default shell in would have
  measured the shell. The `chain` rows still invoke `/bin/sh` — but as the *command under test*,
  named explicitly, because that shell is part of what a client runs.
- **`--input <file>`** — `grim hook run` reads the client payload from stdin, and hyperfine
  otherwise wires stdin to `/dev/null`. An empty payload is not a JSON object, so the runtime would
  refuse (exit 0, nothing spawned) and the row would report **a refusal wearing a dispatch's name**.
- **`--warmup 20`** for the warm rows — takes the page cache and the dynamic loader out of the
  measurement.
- **`--prepare`** for the cold rows — runs before *every* timed run and its own cost is not
  counted, so the eviction is free to be slow.
- **500 warm runs / 150 cold** — so the p99 is the 495th (resp. 148th) sample rather than an
  extrapolation from a handful.

### What "cold" means here, exactly

`--prepare` runs `evict.py`, which calls **`POSIX_FADV_DONTNEED`** (after an `fsync`, since a dirty
page cannot be dropped) on the grim binary, the dispatch table, and the launcher shim. This drops
those files' clean pages without root.

It is worth naming what this replaced. The WP-K stub's harness used `--prepare 'sync'`, and **`sync`
evicts nothing** — it flushes dirty pages. Measured side by side on the identical command:

| cold-row preparation | p50 |
|---|---|
| `--prepare 'sync'` (the stub's) | 4.5 ms — indistinguishable from the warm row |
| `--prepare 'python3 evict.py …'` | 8.2 ms |

So a "cold" row prepared with `sync` was measuring the warm path under another name. The eviction
now used demonstrably works, and the +4.3 ms it exposes is the 26.8 MB binary being re-faulted.

**What it still does not evict:** `/bin/dash`, libc, and the dynamic loader, which are shared with
every other process on the machine — evicting them is neither possible unprivileged nor
representative of anything a user experiences. So "cold" here means **cold grim + cold table + cold
shim**, not cold everything. Stated rather than implied.

### The dispatch-table shape for each row

"No match over a 1-row table" and "no match over a 100-row table" are different measurements, so
every row names its shape. All tables hold **one root** (one armed root on one machine is the real
shape) and every entry carries the required `client: "claude"` field, so every row is armed **for
the invoking client** — a row whose client does not match is invisible to `client_admits` and would
have looked exactly like a fast no-match.

| Scenario | Rows in table | Armed for `claude` at the fired event | What declines |
|---|---|---|---|
| `unknown-root` | 1 | — | the `--root` token names no root in the table |
| `no-match/event` | 1 (at `PreToolUse`) | 0 (fired at `Stop`) | the event |
| `no-match/matcher×1` | 1, `matcher: "Bash"` | 1 | grim's own matcher (`tool_name: "Read"`) |
| `no-match/matcher×10` | **10**, all `matcher: "Bash"` | **10** | grim's own matcher, ten times |
| `match/1` | 1, `matcher: null` | 1 | nothing — payload spawns |

Entry shape follows `test/tests/test_hook_run_runtime.py`'s single `_entry` construction point.
Fixtures are built **directly** rather than by `grim install`, because arming does not converge yet
(WP-J2 proved `sync_for_state`'s body absent) — the launcher shim and registration string are
reproduced byte-for-byte from `src/install/hook_launcher.rs`'s `shim_body` and `registered_command`,
including that claude's verdict-exit-code arm set is empty.

### Known measurement caveats

- **The audit trail grows during a run.** 500 runs × 10 armed hooks appends 5,000 records. The
  trail is reset before the run set, not between the commands inside one `hyperfine` invocation.
  Appends are O(1) and the rotation check is one `statx`, so the effect is small — but it is not
  zero, and later runs in a set append to a larger file.
- **The 9P run took 4 minutes wall-clock** for the same work the ext4 run does in well under one.
- **`strace -c` timings are not used as evidence** — see below.
- The two rows in the platform table marked ¹ are runtime-only, not full-chain.

## How each scenario was confirmed

This is the part that makes the rest readable. **Every refusal on the `grim hook run` path exits 0**
(invariant I3 — some clients fail closed on a non-zero hook exit), so the exit code cannot
distinguish "dispatched nothing because nothing matched" from "refused the argv before reading
anything". A scenario that silently refused would measure process startup and report it as dispatch
latency, and nothing in the timing output would look wrong.

So `task bench:confirm` runs each scenario once against a reset audit trail
(`$GRIM_HOME/hooks/hook_audit.jsonl`, the dispatch table's sibling) and asserts the record count and
outcome. Expected counts come from the runtime's contract, not from observation. **It fails the run
if any scenario does not match**, so the timings cannot be produced without it passing:

```
  ok   unknown-root           exit=0 records=  0 outcomes=[]
  ok   no-match/event         exit=0 records=  0 outcomes=[]
  ok   no-match/matcher x1    exit=0 records=  1 outcomes=['no-match']
  ok   no-match/matcher x10   exit=0 records= 10 outcomes=['no-match']
  ok   match/1                exit=0 records=  1 outcomes=['completed']
every scenario confirmed by its audit trail; the timings below measure what they say
```

Three independent confirmations back the table:

1. **Audit record counts, as above** — and they distinguish exactly the thing that needed
   distinguishing: the two zero-record rows prove the early return really is early (the trail is
   never opened), and the 10-record row proves the ten armed hooks each wrote.
2. **A payload marker.** `match/1`'s payload was replaced by a recording script
   (`cat > spawn.marker`) for one run; the marker appeared and contained the C-002 envelope —
   `{"schema":1,"event":"PreToolUse","native_event":"PreToolUse","client":"claude",
   "scope":"global","hook":"bench/h0","tier":"observer","cwd":"/repo",…,"tool":{"name":"Bash",…`.
   So the match row spawns a real child that receives a real envelope.
3. **Record counts after the timed runs.** The `cwd=9p` control wrote exactly 520 records for
   `matcher×1` and 5,200 for `matcher×10` — 520 invocations (20 warmup + 500 runs) and 520 × 10.
   The counts match the invocation count exactly, so no run in the set silently refused.

## Syscall attribution

`task bench:syscalls` (`strace -c -f`). **The `seconds` column is not read as wall-clock evidence
and no timing claim here rests on it** — `strace` serializes traced threads, which inflates
`futex` and thread-contention time enormously. hyperfine owns the timing; `strace` owns the *where*.
Call counts below, stable across two independent runs:

| path | `clone3` | `execve` | `vfork` | `openat` | `statx` | `newfstatat` | `mkdir` | `write` | **total** |
|---|---|---|---|---|---|---|---|---|---|
| `grim --version` (no runtime built) | 0 | 1 | 0 | 10 | 0 | 3 | 0 | 1 | **91** |
| `no-match/event` (early return) | **24** | 1 | 0 | 15 | 5 | 3 | 0 | 2 | **953** |
| `no-match/matcher×1` | 28 | 1 | 0 | 16 | 7 | 3 | 1 | 2 | **1118** |
| `no-match/matcher×10` | 26 | 1 | 0 | 25 | 25 | 3 | **10** | 11 | **1123** |
| `match/1` | 28 | **25** (23 failing) | 0 | 24 | 8 | 8 | 2 | 4 | **1234** |
| `chain`, `no-match/matcher×1` | 25 | **3** | **1** | 28 | 7 | **15** | 1 | 2 | **1137** |

What it attributes:

- **The no-match cost is process and runtime startup, not dispatch logic.** `grim --version` costs
  **91** syscalls; the cheapest path that reaches `run` costs **953**, of which **24 are `clone3`**
  plus ~150 `futex` and an `epoll` — one worker thread per logical CPU (24 cores, 24 clones).
  File I/O of every kind — the dynamic loader's own library opens included, not just the dispatch
  table — accounts for only about **35** of those ~860 extra syscalls (15 `openat`, 5 `statx`,
  14 `read`); the table itself is one open and its reads.
  So of the 3.20 ms no-match floor, ~1.27 ms is loading a 26.8 MB binary and ~1.9 ms is
  overwhelmingly **building a multi-threaded Tokio runtime**, and the dispatch table read is
  in the noise.
- **G-5's per-hook cost, at the syscall level.** Going from 1 to 10 armed-but-unmatched hooks adds
  **+9 `mkdir`, +9 `openat`, +18 `statx`, +9 `write`** — about four syscalls *and one file open*
  per armed hook, per tool call, linear in the armed count. Every one of the 10 `mkdir` calls
  **fails with `EEXIST`**: `append`'s `create_dir_all` prelude re-runs per record.
- **The B8 fork is exactly one `vfork`**, plus 2 extra `execve`, +12 `newfstatat` and +12 `openat`
  (the two `[ -f ] && [ -x ]` guard pairs and two shell startups). Those four deltas are
  deterministic; the *totals* are not directly differenceable, because the runtime's thread and
  `futex` counts vary run to run (`clone3` came out 25–28 across runs of the same command). The
  fork is real and measurable in wall-clock (+0.75 ms) but it is a couple of dozen syscalls against
  a ~1,100-syscall baseline — it is *not* where the guard path's cost lives.
- **The match path pays a PATH search.** 25 `execve`, **23 of which fail** — the handler argv names
  the interpreter `sh` unqualified, so the spawn probes every `PATH` entry before finding it.

## Findings

Reported, **not fixed** — every file involved belongs to another work package.

### F-1 · The guard path builds a 24-worker Tokio runtime on every tool call — `src/main.rs:172`

`tokio::runtime::Runtime::new()` is the **multi-threaded** scheduler with `worker_threads =
num_cpus`, and it is constructed unconditionally for every subcommand before `app::run` — including
`grim hook run`, which a client invokes once per armed `(client, event)` per tool call. Measured
consequence: **24 `clone3` and ~860 extra syscalls on the cheapest possible no-match**, ~1.9 ms of
the 3.20 ms floor, scaling with the machine's core count (so *worse* on a bigger machine). The
dispatcher's own work is one file read and, at most, a few appends; it awaits nothing concurrently
on the guard path. A `current_thread` runtime for this one arm would remove most of the floor, which
is the largest single win available on this path. **Owner: not mine — `src/main.rs` is outside my
file set.** Flagged for whoever owns `main.rs`/`app.rs`.

### F-2 · G-5 is negligible on ext4 and pathological on 9P — decision input

The implement-report flagged G-5 (every armed-but-unmatched hook writes a `NoMatch` record on every
tool call) as a real cost worth a decision. The numbers make that decision concrete, and it depends
entirely on the filesystem:

| armed-but-unmatched hooks | ext4 p50 | 9P p50 | 9P p99 |
|---|---|---|---|
| 1 | 3.56 ms | 15.29 ms | 33.02 ms |
| 10 | 3.95 ms | **142.07 ms** | **295.51 ms** |
| marginal cost per hook | **+0.04 ms** | **+14.1 ms** | — |

On a native filesystem G-5 costs ~40 µs per armed hook and the doc's forensic argument
("the guardrail did not apply here" is the answer to the most common forensic question) buys it
cheaply. On a 9P `$GRIM_HOME`, **ten armed hooks make every single tool call cost 142 ms at p50 and
296 ms at p99** — an agent doing a hundred tool calls pays 14 seconds of pure audit bookkeeping for
hooks that all declined. A measured `stat()` on this machine's `/mnt/c` is **958 µs p50 / 3.37 ms
p99**, against **0.9 µs / 4.3 µs** on ext4.

This does not settle the keep-or-drop question, and it is not mine to settle. It does mean the
answer should not be filesystem-blind. Two cheaper shapes than dropping the record outright, both
suggested by the attribution rather than by preference: hoist `create_dir_all` and the trail open
out of the per-record path (the 10 `EEXIST` `mkdir` calls and 10 separate opens are per-record work
that one open per invocation would cover), or batch one invocation's `NoMatch` records into a single
append. Either keeps the forensic answer and removes the linear syscall count.

### F-3 · The matched path resolves its interpreter through `PATH` — 23 failing `execve` per spawn

The handler argv names `sh` unqualified, so each matched dispatch probes every `PATH` entry. It is
~0.1 ms of the 0.57 ms spawn cost on ext4 and it is not a correctness problem — but on 9P each
failed probe is a 9P round trip, and it is a `PATH`-dependent execution decision on a path whose
whole design otherwise avoids environment-derived resolution (compare W9, which deliberately gives
the shim no `$PATH` fallback). Noted for the owner of the handler-argv contract; no defect claimed.

### F-4 · The stub harness's cold rows measured the warm path

`--prepare 'sync'` evicts nothing (4.5 ms, vs 4.4 ms warm). Fixed in this commit — the harness now
evicts with `POSIX_FADV_DONTNEED` and the cold rows separate from the warm ones by ~4.3 ms. Recorded
because the same mistake reads as plausible in any future benchmark.

### Not a finding: nothing arms yet

The fixtures are built directly because `sync_for_state`'s convergence body does not exist (WP-J2).
That is WP-R's concurrent work, and **no conclusion here treats it as a runtime defect** — the
runtime dispatched correctly against every hand-built table, which is what these rows measure.

## Reproducing this

```sh
task bench:confirm                                  # the credibility check, on its own
task bench:hooks                                    # the whole matrix for this host + filesystem
task bench:hooks BENCH_ROOT=/mnt/c/temp/grim-bench   # the 9P row (WSL2)
task bench:syscalls                                 # strace -c attribution (needs strace)
task bench:clean
```

`report` stamps the platform, the filesystem of `BENCH_ROOT`, the grim version and the binary
profile into its own output, so a pasted result identifies itself and a debug build cannot be
mistaken for a release one.

**Note on `strace` for this session:** `strace` was absent from this machine and there is no
passwordless root, so it was obtained unprivileged with
`apt-get download strace libunwind8 && dpkg-deb -x … ./straceroot`, then run with
`LD_LIBRARY_PATH` pointed at the extracted `libunwind-ptrace.so.0`. `task bench:syscalls` expects
`strace` on `PATH` and refuses with an install hint otherwise; it does not depend on that
workaround.

## After WP-U — what the fixes moved

**Appended, not merged: every number above is the baseline and none of it was edited.** WP-U fixed
F-1 (the runtime flavor) and F-2 (the per-record audit open) and left F-3 alone; this section is
its after-column. Full method, the variant experiment behind the F-1 trade, and the F-3 decision:
[`wp-u-report.md`](./wp-u-report.md).

**These rows are NOT comparable to the ones above.** They were measured in a different worktree
(`.agents/worktrees/wp5-u`) on the same host under different load, and the same harness at the same
commit reproduced the baseline's `matcher×10` 9P row within 2 % but its ext4 warm rows ~9 % higher.
So the before-column below is a **fresh baseline taken in that tree at `31a9154`**, and every delta
is within-tree. Same harness, same flags, same fixtures: `task bench:hooks` and
`task bench:hooks BENCH_ROOT=/mnt/c/temp/grim-bench-wp5u`.

### `$GRIM_HOME` on ext4

```
scenario · chain · thermal                  runs    p50    p99  p50 was   Δp50  p99 was   Δp99
------------------------------------------------------------------------------------------------
unknown-root · runtime · warm                500   1.46   3.21     3.49  -2.03     4.92  -1.70
unknown-root · runtime · cold                150  13.17  15.66     8.92  +4.25    11.45  +4.21
no-match/event · runtime · warm              500   1.44   3.25     3.40  -1.96     4.84  -1.59
no-match/event · runtime · cold              150  13.25  15.34     8.71  +4.53    10.80  +4.54
no-match/matcher×1 · runtime · warm          500   1.96   3.98     3.78  -1.82     5.26  -1.27
no-match/matcher×1 · runtime · cold          150  13.92  16.33     9.02  +4.90    11.52  +4.81
no-match/matcher×1 · chain · warm            500   2.65   4.96     4.68  -2.03     6.74  -1.78
no-match/matcher×1 · chain · cold            150  14.93  17.93    10.03  +4.89    12.47  +5.47
no-match/matcher×10 · runtime · warm         500   1.95   3.88     4.01  -2.06     5.78  -1.90
no-match/matcher×10 · runtime · cold         150  13.77  16.97     9.67  +4.10    12.72  +4.25
match/1 · runtime · warm                     500   2.50   4.31     4.29  -1.79     6.56  -2.25
match/1 · runtime · cold                     150  14.78  17.48     9.60  +5.18    12.26  +5.22
match/1 · chain · warm                       500   3.16   5.79     5.04  -1.88     7.44  -1.66
match/1 · chain · cold                       150  16.05  19.06    10.77  +5.28    13.24  +5.82
```

### `$GRIM_HOME` on `/mnt/c` — the 9P row

```
scenario · chain · thermal                  runs    p50    p99  p50 was     Δp50  p99 was     Δp99
------------------------------------------------------------------------------------------------
unknown-root · runtime · warm                500   4.54   8.85     5.66    -1.12    33.62   -24.77
unknown-root · runtime · cold                150  16.46  22.88    11.51    +4.96    38.03   -15.16
no-match/event · runtime · warm              500   4.39   6.58     5.61    -1.22    23.20   -16.63
no-match/event · runtime · cold              150  16.60  22.41    10.88    +5.72    21.86    +0.55
no-match/matcher×1 · runtime · warm          500  11.57  44.44    14.75    -3.18   175.54  -131.09
no-match/matcher×1 · runtime · cold          150  21.21  40.06    15.63    +5.59   105.76   -65.70
no-match/matcher×1 · chain · warm            500  18.67  52.34    24.33    -5.66   156.71  -104.37
no-match/matcher×1 · chain · cold            150  31.38  91.93    27.69    +3.69    71.04   +20.89
no-match/matcher×10 · runtime · warm         500  10.44  22.69   139.21  -128.77   316.44  -293.75
no-match/matcher×10 · runtime · cold         150  20.91  32.28   148.21  -127.30   304.83  -272.55
match/1 · runtime · warm                     500  22.71  42.25    19.32    +3.38   153.38  -111.13
match/1 · runtime · cold                     150  24.60  77.57    19.73    +4.87    59.40   +18.17
match/1 · chain · warm                       500  24.11  48.34    27.29    -3.19   170.36  -122.02
match/1 · chain · cold                       150  31.14  51.37    30.71    +0.42   148.17   -96.80
```

The 9P warning above still applies: cold and warm are one population there, and a ±3 ms p50 move on
a row whose p99 was 150 ms is noise. The row that is **not** noise is `matcher×10`.

### The two fixes, separated

| | ext4 warm p50 | 9P warm p50 | marginal cost per armed-but-unmatched hook |
|---|---|---|---|
| baseline (`31a9154`) | 4.01 | 139.21 | **+0.042 ms** ext4 · **+13.8 ms** 9P |
| after F-1 (current-thread runtime) | 2.19 | — | +0.042 ms ext4 (unchanged — F-1 is not about I/O) |
| after F-1 + F-2 (batched append) | **1.95** | **10.44** | **~0.000 ms** on both |

`matcher×10` rows; the marginal cost is `(p50(×10) − p50(×1)) / 9`.

### Syscall attribution, after

`task bench:syscalls`, same `strace -c -f`, counts stable across two runs:

| path | `clone3` | `execve` | `openat` | `statx` | `mkdir` | `write` | **total** |
|---|---|---|---|---|---|---|---|
| `no-match/event` (early return) | **0** (was 24) | 1 | 11 (was 15) | 2 (was 5) | 0 | 0 (was 2) | **106** (was 953–974) |
| `no-match/matcher×1` | 4 (was 26) | 1 | 12 (was 16) | 4 (was 7) | 1 | 7 | **255** (was 1058–1118) |
| `no-match/matcher×10` | 4 (was 28) | 1 | **12** (was 25) | **4** (was 25) | **1** (was 10) | 7 (was 11) | **249** (was 1123–1212) |
| `match/1` | 2 (was 28) | **25, 23 failing — unchanged (F-3)** | 20 (was 24) | 5 (was 8) | 2 | 9 | **302** (was 1210–1234) |
| `chain`, `no-match/matcher×1` | 1 (was 25) | 3 | 24 (was 28) | 4 (was 7) | 1 | 4 | **289** (was 1137–1162) |

Two readings worth stating. The cheapest guard path now makes **no `clone3` at all** — a
current-thread runtime starts no workers, and the blocking pool is only spun up when a record is
actually written, which that path never does. And `matcher×10` now costs what `matcher×1` costs on
every file-touching column: that is what "the linear term is gone" means, and it is the syscall-level
statement of the 9P row above.

### What did NOT move, and why it is here

- **F-3 is unchanged and deliberately so** — the match path still spends 23 failing `execve`
  resolving `sh` through `PATH`. The reasoning is in `wp-u-report.md`; it is a contract decision,
  not a measurement one.
- **The cold rows got worse by ~5 ms**, on ext4 and 9P alike, and that is F-1's own doing: the
  24-worker pool had been faulting the 26.8 MB binary back in parallel. It is a real regression on
  the first call after an idle period, taken knowingly against a −2 ms win on every call after it.
  The variant table (1, 2, 6, 24 workers × cold/warm) that establishes the mechanism is in
  `wp-u-report.md`.
