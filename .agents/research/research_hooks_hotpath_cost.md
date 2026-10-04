# Research: Trampoline hot-path cost budget and design patterns

## Metadata

**Date:** 2026-08-14
**Domain:** performance | CLI design | packaging
**Triggered by:** hex-architect tier-high research axis `trampoline-hot-path-cost`,
spawned alongside [`research_hooks_trampoline.md`](./research_hooks_trampoline.md)
(F5: "Latency budget: the trampoline is on the hot path"). That file asserts
`grim hook run` must be a fast path but defers the actual budget number to
measurement. This file supplies the budget *shape* and the design patterns
that keep a two-process-spawn-per-tool-call trampoline cheap, without
inventing a target number.
**Expires:** 2027-02-14 (co-expires with the trampoline research; re-verify
process-spawn numbers if the implementation task lands on different
hardware/OS mix than assumed here)
**Companion artifacts:**
[`research_hooks_trampoline.md`](./research_hooks_trampoline.md) (grim-side
design), [`research_hooks_vendor_survey.md`](./research_hooks_vendor_survey.md)
(17-client survey)

## Direct Answer

**No number is fabricated here — that is explicitly deferred to an
implementation/measurement task.** What follows is the *shape* of the
budget and the pattern that keeps it small.

Recommended fast-path architecture in one sentence: **`grim hook run` must
skip `Context`'s normal command path entirely (no config walk-up, no
`grimoire.toml`/`grimoire.lock` parse, no install-state JSON parse, no OCI
client) and instead read one small pre-resolved dispatch file written at
install time by `sync_config`, match the event against it in memory, and
exit — so the invocation's own cost is dominated by process-spawn floor and
a sub-millisecond file read, not by anything proportional to catalog or
lock size.**

Every precedent surveyed below (`lefthook`, `starship`, `mise`/`rtx`,
`eslint_d`, `direnv`) converges on the same three techniques, in order of
value: (1) do no work proportional to installed-artifact count on the hot
path — precompute it once at install/config-change time instead; (2) avoid
interpreter/runtime startup (Python, Node) by shipping a compiled,
statically-or-near-statically-linked binary; (3) treat a persistent daemon
as a last resort reached only after a compiled fast path is measured and
still insufficient — several ecosystems (`pnpm server`, in-repo YAGNI note
on the trampoline itself) have shipped and then removed exactly this
escape hatch.

**What the implementation task must measure**, once `grim hook run` exists:

1. **Two numbers, not one** — the *no-match* fast path (read dispatch file,
   match nothing, exit 0) and the *match-and-dispatch* path (same, plus
   spawning the user's payload). The trampoline's own tax is the first
   number; the second is what the user actually experiences per tool call.
2. **p50 and p99, not mean** — process-spawn cost is bimodal (page-cache
   warm vs cold, antivirus scan vs cached verdict on Windows), and an
   agent loop cares about tail latency because it fires hundreds of times
   per session.
3. **Cross-platform**, not just Linux — Linux/macOS/Windows/WSL2 native and
   WSL2-with-Windows-filesystem separately (see Q2), because the ratios
   differ by roughly an order of magnitude between them.
4. **Cold vs warm**, both intentionally — first invocation after
   `grim install` (cold page cache, cold antivirus verdict) and steady
   -state repeated invocation (hot cache) are both real user experiences.
5. **With `hyperfine`** (`--warmup`, `--min-runs`, exported JSON) on
   Linux/macOS, and a documented Windows-equivalent methodology since
   `hyperfine`'s shell-subtraction calibration behaves differently there
   (see Q7) — plus `strace -c` / `perf stat` on Linux to attribute the
   syscall cost, not just the wall-clock number.

---

## Q1 — Precedent: tools that shell out per event on a hot path

| Tool | Language/runtime | Hot-path shape | What it publishes about startup | Concrete technique |
|---|---|---|---|---|
| **lefthook** | Go, single static binary | Runs once per `git` hook event, reads `lefthook.yml`, fans out to N commands (optionally in parallel) | **Publishes no numbers.** Its own GitHub wiki page titled "Benchmark lefthook vs pre-commit" (fetched 2026-08-14) contains marketing prose ("Fast. It is written in Go.") and feature descriptions, but zero comparative timing data, no methodology, no ms figures `[unofficial finding: page is undocumented on the metric it is named for]` | Compiled dispatch: no interpreter startup, single YAML parse per invocation, no work proportional to repo size until a hook actually runs |
| **starship** | Rust, single binary, tokio async runtime internally | Rendered once per shell prompt (i.e., far more often than a git hook) | Documents a `command_timeout` config to cap per-module cost and shows a per-module timing breakdown in its own debug output (module times from <1ms to 112ms observed in one user's post) `[unofficial: forum/GitHub post, no formal methodology]`. A published note states prompts should render under 200 ms as a UX guideline `[unofficial]` | (a) modules run **concurrently** on tokio rather than sequentially — the async runtime buys parallel I/O, not a hot-path bypass of construction cost; (b) a **timeout per module** bounds worst-case tail latency; (c) **caching of git operations within one render pass** avoids re-running `git status`/`git log` per module. Independent large-repo tests found starship could hit 104 ms (Rust repo) to 2028 ms (Chromium) driven almost entirely by `git status` cost in `vcs_info`, while a config-cache-heavy competitor (Powerlevel10k, Zsh+C) stayed under 10 ms by memoizing repo state across prompts — i.e., **the bottleneck class that matters is external commands invoked per render, not binary startup itself** `[unofficial: romkatv/IlanCosman comparison threads, no shared methodology, dates 2019-2020, treat as indicative only]` |
| **pre-commit** | Python | Runs once per `git commit`, plus historically once per configured hook (each hook = its own Python venv invocation) | GitHub issue #1069 ("Performance issues & overhaul proposal") documents ~50 ms per hook from interpreter/venv overhead, ~600 ms total across a typical hook set `[unofficial: GitHub issue, self-reported, no formal benchmark harness]` | The **negative** example: per-hook Python process + per-hook virtualenv activation is the interpreter-startup tax this whole research axis exists to avoid. Its community successor `prek` (Rust rewrite) is cited specifically as removing this tax `[unofficial mention, not independently verified here]` |
| **husky** | Node.js/shell shim | Runs once per git hook event, delegates to `lint-staged`/user script | Husky's own docs (fetched via search, not directly re-verified with a WebFetch in this session) state the shim itself runs in ~1 ms and is 2 kB — i.e., husky's *own* code is not the cost. The cost users report is the **downstream** Node.js runtime startup for whatever husky invokes (~ent Node cold start), which is a different problem than husky's design `[unofficial]` | Demonstrates the same lesson as pre-commit from the other direction: the trampoline layer itself can be near-zero-cost; the tax comes from what it execs next. This is directly analogous to grim's situation — `grim hook run`'s own budget matters, but so does what it then spawns |
| **git hooks (native)** | Whatever the hook script declares (`#!` shebang) | Git execs the hook file directly if executable, else interprets with `sh` | `git-scm.com/docs/githooks` (canonical spec) states the invocation contract but publishes no performance numbers — git's own hook dispatch is "whatever `execve` costs for that one file," with no intermediate config parse at all | This is the **zero-overhead baseline**: git does no matching, no config read, no dispatch table — the filename **is** the identity and the only "index" is the directory listing. Grim's trampoline necessarily adds a config-read + match step on top of this baseline, which is the entire reason F5 exists |
| **direnv** | Go, single static binary | Runs once per shell prompt (like starship, more frequent than a git hook) | direnv's own site states it is "compiled into a single static executable" and fast enough to be "unnoticeable" on every prompt; no ms figure published in the fetched material `[unofficial: marketing copy, not a benchmark]` | (a) **stateless single-binary check** — before each prompt it checks for `.envrc` existence up the directory tree and only does real work (spawns a `bash` subshell to source `.envrc`) when something changed; (b) **explicit design position: no daemon** — the community's own caching add-ons (`nix-direnv`, `asdf-direnv`) exist precisely because direnv chose not to build one in, instead exposing a file-based cache contract third parties fill in. This is the direct precedent for Q5/Q6: a fast compiled binary doing a cheap existence/mtime check *is* the accepted idiom, and caching is layered on top only where a specific backend (Nix) makes it worthwhile |

**Cross-cutting pattern across all five:** none of the compiled-binary tools
(lefthook, starship, direnv, and grim itself) publish a hard startup-time
number with stated methodology. The absence of a number is itself the
signal that "unmeasurably fast relative to what it's compared against" is
the accepted bar in this class of tool — nobody benchmarks single-digit
milliseconds against nothing, they benchmark against the *next* worst
alternative (Python/Node interpreter startup, or an external command like
`git status`). Grim's trampoline should adopt the same framing: the number
that matters is relative to (a) the payload it spawns and (b) the coding
agent's own per-tool-call overhead, not an absolute target invented here.

## Q2 — Process-spawn cost, platform by platform

**Primary source:** bitsnbites.eu, "Benchmarking OS primitives" (fetched
2026-08-14) — a C micro-benchmark comparing thread creation, process
creation (`fork`+`waitpid`, Linux/macOS only — the author states Windows
"does not have any corresponding functionality" for a bare fork-style
test), and full program launch (fork+exec vs `CreateProcess`) across Linux,
macOS, and Windows on comparable hardware. `[unofficial: single-author
benchmark repo, methodology described but not independently reproduced
here — treat ratios as indicative, not authoritative]`

| Comparison | Finding | Source |
|---|---|---|
| Thread creation | Linux ~3x faster than Windows; macOS ~2x faster than Windows | bitsnbites.eu |
| Process creation (fork) vs thread creation, same OS | Linux: process ~2-3x the cost of a thread; macOS: ~7-8x | bitsnbites.eu |
| **Full program launch** (fork+exec / CreateProcess) | **Linux ~10x faster than macOS, >20x faster than Windows** on the same task; "even a Raspberry Pi 3 is faster than a stock Windows 10 Pro install on an octa-core Ryzen 1800X" for this specific operation | bitsnbites.eu |
| Windows-specific variance | Program-launch benchmark on Windows is "very sensitive to background services such as Windows Defender and other antivirus software" | bitsnbites.eu |
| Windows CreateProcess absolute range | General guidance (not the same benchmark) puts full `CreateProcess` process creation in the **10-100 ms** range depending on DLL dependency count; a minimal console app with few dependencies sits at the low end | `[unofficial]` blog/forum synthesis, no single authoritative source found — reported here as a *range*, not a number to design against |

**WSL2 specifically:** a maintainer comment on a WSL-kernel-performance
issue (Locietta/xanmod-kernel-WSL2#8, fetched via search, not independently
re-verified) and a BuildStream issue thread report native Linux `fork`
around **~30 µs**, versus **2-5 ms for a simple process fork under WSL2**,
and up to ~50 ms for a heavier fork inside a real build tool `[unofficial:
GitHub issue comments, self-reported numbers, no shared harness]`. That is
roughly **two orders of magnitude slower than native Linux fork**, though
still far cheaper than the Windows-native `CreateProcess` numbers above —
WSL2's process creation goes through `lxss.sys` translating the Linux
`fork`/`clone` syscall into NT process-creation primitives, which is a
different code path from a native Windows exe launch. **Practical
implication for grim:** WSL2 is not a proxy for either "Linux" or "Windows"
numbers — it must be measured as its own third platform, and specifically
distinguished from WSL2-processes-touching-a-Windows-mounted-filesystem
(the 9P-protocol path), which is a *separate*, much larger tax (a `stat()`
across the 9P boundary was measured at ~6 ms in one report — irrelevant to
process spawn itself but highly relevant if the dispatch-table file or the
hook artifact happens to live under `/mnt/c/...`).

**What dominates a cold Rust binary's startup**, ranked by the material
found (Q3 has the Rust-specific detail): dynamic linker resolution (glibc
dynamic linking pulls in the loader + shared libc, adding syscalls beyond
the process's own logic — one analysis counted ~35 syscalls for a
dynamically-linked "hello world" using `printf`/libc, versus a handful for
a `#![no_std]`-style minimal binary), then the OS-level process-creation
primitive itself (`fork`+`exec` / `CreateProcess`), then the program's own
`main`. **No source gave an absolute microsecond number for "glibc dynamic
link resolution alone" isolated from process creation** — `NOT DOCUMENTED`
at the precision this axis wants; the implementation task should get this
number itself via `strace -c` (Linux) rather than trust a web estimate.

## Q3 — What makes a Rust CLI slow to start, and where grim's own startup sits

**Concretely, avoid on a hot-path binary:**

- **Large `lazy_static`/`OnceLock` graphs evaluated eagerly** — not a
  startup cost by themselves (both are lazy by construction — the whole
  point of `lazy_static`/`OnceLock` is deferred initialization on first
  access), but a hot-path command that *touches* many of them for the
  first time pays their cumulative init cost inline. The risk is
  transitive: a shared "eager-looking" module accessed once pulls in a
  much larger graph than the command needed.
- **`serde` parse of a large document** — proportional to document size;
  cheap for grim's own install-state/lock files at their current scale
  (kilobytes), but a design that reads the *whole* lock or *whole*
  install-state file on the hot path re-pays a cost that scales with the
  number of installed artifacts, not with whether this particular event
  has a matching hook. This is exactly the shape Q4's dispatch table is
  built to avoid.
- **TLS/crypto init and regex compilation** — both are one-time,
  amortizable costs the `regex` crate's own performance notes describe as
  "quite expensive" relative to a single match, explicitly recommending
  `lazy_static`/`OnceLock` to pay the cost once per process rather than
  per call; compile time is reported as ranging from "a few dozen
  microseconds" for simple patterns to tens of milliseconds for
  Unicode-heavy patterns (`\pL{100}` cited at ~44 ms in the crate's own
  `PERFORMANCE.md`) `[crate docs, treated as authoritative for the regex
  crate itself]`. **Implication for the trampoline: it should need zero
  regex compilation on the no-match path** — matching against a
  pre-resolved dispatch table should be exact/glob-precompiled-at-install-
  time string comparison, not a freshly-compiled regex per invocation.
- **Tokio runtime construction** — search material did not surface an
  isolated microsecond figure for `Runtime::new()` itself (the available
  material discusses *task-scheduling* overhead — recommending async work
  units stay above 10-100 µs between `.await` points to avoid scheduler
  overhead dominating — not construction cost) — `NOT DOCUMENTED` at the
  precision needed; treat as "non-zero, needs measuring" rather than
  assuming it is negligible. The actionable design conclusion holds
  regardless of the exact number: **a fast-path subcommand that needs no
  async I/O should not construct a tokio runtime at all**, using a
  `#[tokio::main(flavor = "current_thread")]`-free plain synchronous
  `fn main` branch (or an early synchronous return before entering the
  async command dispatcher) for `grim hook run`'s no-match path.
- **Directory walking and file locks** — grim's own `walk_up_for_config`
  (`src/config/project_config.rs:149`) ascends the directory tree from cwd
  looking for `grimoire.toml`; cost is proportional to directory depth
  (cheap in absolute terms — a handful of `stat`-class syscalls — but not
  zero, and each syscall crossing the WSL2 9P boundary is disproportionately
  expensive per Q2). Advisory file locks (used elsewhere in grim for
  read-modify-write config/lock operations) are unnecessary on a read-only
  hot path and must not be acquired there.

**Where grim's actual startup cost lives today** (verified by reading
`src/context.rs` and `src/config/project_config.rs` in this session,
2026-08-14, at commit `03e59b0`): **`Context::new()` itself is already
cheap** — it does env-var reads only (`GRIM_HOME`, `GRIM_DEFAULT_REGISTRY`,
`GRIM_OFFLINE`), no I/O, no parsing, and defers the OCI-access client
construction to first use via a `OnceLock`-backed memo (`Context::access`).
The framing "grim's normal startup builds a `Context`: config discovery by
walk-up, lock parse, install-state parse" describes the **per-command scope
resolution** most subcommands perform *after* `Context::new()`, not
`Context::new()` itself — `walk_up_for_config` (directory ascent for
`grimoire.toml`), the subsequent TOML parse of that config, the
`grimoire.lock` parse, and the project install-state JSON parse at
`<workspace>/.grimoire/state.json`. **This is good news for the trampoline
design**: `grim hook run` does not need to fight `Context::new()` (it is
already fast) — it needs to be wired as a command that **skips the normal
per-command scope-resolution call entirely**, reading only its own
dispatch file (Q4) instead. This is architecturally the same shape as
`grim schema`/`grim completions`, which are already documented
(`subsystem-cli-api.md` § "Payload-Plain Reports" / "Commands That Exec a
Child Process") as exempt from the normal `Context`-driven command flow.

**Standard pattern for a "fast path" subcommand**: dispatch on the raw
argv/subcommand name *before* constructing the full command-line parser's
shared context, so the fast path's binary-size and initialization surface
is the smallest slice of the program the router can reach — precisely the
`grim schema` / `grim completions` precedent already in this codebase,
generalized to `grim hook run`.

## Q4 — Compiled dispatch table design: prior art and shape

**Prior art for "resolve once at install time, read cheaply at hot-path
time":**

- **rbenv/pyenv/asdf shims** — a directory of tiny wrapper
  scripts/executables, one per shimmed command name, each of which does a
  minimal version-file lookup and `exec`s the real interpreter. This is
  the closest existing shape to "pre-resolved dispatch, one small read on
  the hot path" — but it pays a **shim-per-invocation cost**, reported at
  roughly **~120 ms added per command execution for asdf** `[unofficial:
  aggregated in mise's own comparison material, exact methodology not
  re-verified independently]`.
- **mise (formerly rtx)** — the direct answer to why asdf's shim design is
  slow: mise is a single static Rust binary that **manipulates `$PATH`
  directly** instead of installing a shim per tool, so `which node`
  resolves straight to the real binary with zero indirection at
  invocation time. Reported overhead when switching directories / showing
  the prompt is **~5-10 ms**, versus asdf's ~120 ms per command — a
  reported **20x-200x** improvement `[unofficial: mise's own comparison
  docs and third-party summaries, not independently re-benchmarked in
  this session]`. **This is the most relevant prior art for Q4/Q6**: mise
  demonstrates that the *cheapest* correct dispatch is not "read a file
  and match" at all, but "make the OS-level resolution mechanism itself
  point at the answer" — for grim's case, the closest analogue would be
  registering the trampoline command directly rather than needing any
  runtime matching, but the trampoline still needs to decide *which
  installed hook(s)* apply for a given `(client, event)`, so grim cannot
  fully eliminate a read+match step the way mise eliminates its shim.
  What grim *can* copy is mise's core discipline: **push the expensive
  resolution work (which vendor supports what, which hooks are declared,
  matcher compilation) to install/config-change time, and make the
  runtime path a lookup, not a computation.**
- **eslint_d** — not a dispatch-table pattern but a directly relevant data
  point on interpreter-startup elimination: a background Node server
  brings a ~700 ms cold ESLint invocation down to ~160 ms by skipping
  repeated Node startup, and down to <50 ms when talking to the server
  over a raw socket (bypassing even the `eslint_d` CLI wrapper's own
  Node startup) `[unofficial: project README/npm page, self-reported]`.
  This quantifies the daemon's *specific* win (interpreter startup
  removal) — a number that does not transfer to grim, whose baseline is
  already a compiled binary with no interpreter to eliminate.

**Shape recommendation for grim's dispatch table:**

- **Format: a small, install-time-generated JSON (or equivalent flat)
  file**, not a binary/mmap format, at this stage. Reasoning: Q3 already
  establishes that JSON parse cost is proportional to *document size*, and
  the dispatch table's size is bounded by "number of registered
  `(client, event)` entries times a small per-entry record" — realistically
  low kilobytes even for a large install. A binary/mmap format would only
  earn its complexity if measurement shows JSON parsing is a *material*
  fraction of the no-match path's total cost, which every precedent above
  suggests it will not be, because process-spawn cost dominates by at
  least an order of magnitude (Q2) before any bytes are read. This is a
  design decision the implementation task should **revisit only if
  measurement contradicts it**, not decide by fiat: is JSON parse of a
  ~1-20 KB file ever the bottleneck? **No source found says yes for a
  file this size, on any platform** — every relevant number in this
  research (process spawn floor: hundreds of µs to tens of ms; JSON parse
  of kilobytes: sub-millisecond in every mainstream serde benchmark
  encountered, though no isolated number was pulled for this specific
  claim — flagged `NOT DOCUMENTED` at that precision, treat as
  "overwhelmingly likely negligible, verify once, then stop worrying
  about it").
- **Invalidation/staleness strategy**: write the dispatch table
  transactionally as part of the same `sync_config` convergence pass that
  already runs after every install/update/uninstall
  (`Vendor::sync_config`, `src/install/vendor.rs:377` per
  `research_hooks_trampoline.md` F2) — there is no separate staleness
  problem to solve because the table's writer is the same code path that
  already reconciles vendor config on every mutating command. The
  dispatch table becomes another output of that convergence, at the same
  trust level as the vendor config entries it is paired with.
- **Correctness when the source of truth changes**: because the table is
  regenerated wholesale on every `sync_config` pass rather than
  incrementally patched, there is no drift window to reason about beyond
  the same window `sync_config`'s other outputs already have (documented
  in `adr_install_state_portability.md`'s "reap window" language) — a
  concurrent reader mid-regeneration should see either the old table or
  the new one, never a torn write; the file write should go through
  grim's existing atomic-write primitive (already used for install state)
  rather than a new mechanism.

## Q5 — When a daemon becomes justified

**Threshold used by comparable tools, and what they paid:**

- **eslint_d** justifies a daemon specifically to eliminate **Node.js
  interpreter startup** (~700 ms → ~160 ms, further to <50 ms bypassing
  even its own CLI). The threshold there is "interpreter startup is the
  dominant cost and cannot be compiled away" — not applicable to grim,
  which has no interpreter to eliminate.
- **rust-analyzer** is architecturally a persistent daemon (an LSP server)
  by necessity, not by an incremental optimization decision — the
  workload (whole-project semantic analysis, sub-100ms response to every
  keystroke) is fundamentally incompatible with a from-scratch process per
  request. Reported startup for the *daemon itself* is 1-2 minutes on a
  real project `[unofficial: rust users forum thread]` — illustrating the
  lifecycle cost side of the trade: a daemon that is expensive to start
  must then justify amortizing that cost across a long resident lifetime,
  which introduces the stale-state and restart-signal problems Q5 asks
  about.
- **pnpm** shipped a background "pnpm server" (a persistent store/lockfile
  server, analogous in spirit to what a hook-dispatch daemon would be) and
  **removed it in pnpm 11** (2026). The release notes (fetched directly,
  2026-08-14) list the removal under "Other removals" with **no stated
  reason** — `[unofficial: absence of justification is itself the
  finding]`. This is the closest confirmed **"tried a daemon and
  reverted"** case surfaced in this research, though the *why* is
  undocumented — the implementation task should not over-read pnpm's
  motive, only note that a well-resourced tool built exactly this kind of
  persistent background helper and later removed it, which is at minimum
  evidence that the lifecycle/complexity cost of a daemon is real enough
  for a major tool to walk back.
- **direnv** is the explicit "considered the shape, chose not to build it
  in" case: its own maintainers point community daemon-adjacent caching
  add-ons (`nix-direnv`) at the specific backend (Nix evaluation) that is
  slow enough to justify caching, while direnv's own core stays a
  stateless single-binary check on every prompt. This maps directly onto
  grim's situation: the *common* case (no hooks registered, or a cheap
  dispatch-table read) should never need a daemon; a daemon, if ever
  justified, should be scoped to the specific expensive sub-case (e.g., a
  hook payload with heavy startup cost of its own), not to the trampoline
  itself.

**What a daemon costs, as a checklist drawn from the above plus general
knowledge of the pattern** (lifecycle, stale state, security surface,
Windows service story — the four dimensions the task asked about):

| Dimension | Cost |
|---|---|
| Lifecycle | Needs a start trigger (lazy-spawn-on-first-use vs. install-time-registered service), a liveness/health check, and a shutdown/idle-timeout policy — none of which a one-shot CLI needs |
| Stale state | A daemon that caches the dispatch table in memory can serve a stale answer after `grim install`/`update` changes it, unless it watches the file or is explicitly restarted — reintroducing exactly the staleness problem Q4's "regenerate wholesale" design avoids for the file-based approach |
| Security surface | A resident process accepting IPC (socket/named pipe) from arbitrary local callers is a new local attack surface that a stateless CLI invocation is not — directly relevant given `research_hooks_trampoline.md`'s finding that hooks are "code that executes automatically... at user privilege" |
| Windows service story | Not found in any source consulted this session — `NOT DOCUMENTED`. Windows daemonization conventionally means either a registered Service (install/uninstall lifecycle, elevated permissions to register) or a per-user background process with its own restart-on-crash story; neither was addressed by any source found, and this is a real gap the implementation task should research directly if a daemon is ever seriously proposed |

**Recommended stance, consistent with the in-repo finding**: the trampoline
research (`research_hooks_trampoline.md` F5) already states "a resident
daemon is the wrong first answer (new lifecycle + new security surface,
and YAGNI) — but it is the escape hatch if measurement demands one." Every
comparable tool surveyed here reinforces that ordering: build the compiled
fast path first, measure it, and reach for a daemon only if the measured
number is dominated by something a daemon actually fixes (which, per every
precedent found, is interpreter startup or expensive external-command
cost — neither of which applies to a from-scratch `grim hook run` reading a
small file).

## Q6 — The no-match fast path: cheapest correct shape

No source found describes a tool that short-circuits **before** reading
its config file via an env var or marker file specifically for a
per-event hot path (this exact pattern — "skip the read entirely when
nothing is registered" — was not documented by any precedent surveyed).
What the precedents *do* establish, which composes into an answer:

- **git hooks' own baseline** (Q1) is the cheapest possible shape:
  `execve` the hook file only if it exists at all, with **no config read
  step whatsoever** when the hook file is absent — git doesn't even fork a
  shell in that case. This sets the ceiling grim cannot reach (grim's
  registration model — one dispatcher entry per `(client, event)`,
  `research_hooks_trampoline.md` D1 — means the trampoline is *always*
  invoked once registered, even for events with zero matching hooks, by
  design; that cost is the deliberate trade for owning matching/ordering
  centrally).
- **direnv's existence-check pattern** is the nearest real analogue: check
  for the cheapest possible signal of "is there anything to do here" before
  doing the expensive part. For grim, the equivalent is: the dispatch-table
  read itself should be the cheap check — a **single small file read**
  (not a directory walk, not a multi-file parse), and a miss (file absent,
  or present but no entry for this `(client, event)`) should return exit 0
  **before** any further I/O.
- **A marker file or env var *ahead* of even that read** is plausible as a
  further optimization (e.g., the vendor registration itself could carry a
  flag telling the trampoline whether *any* hooks exist for that client
  before even naming the dispatch file) but this is speculative — flagged
  as **a design option worth prototyping, not a documented pattern any
  precedent tool actually uses.** The measured cost of a single small file
  read (Q4) is expected to be negligible next to process-spawn cost (Q2),
  which is the stronger argument for *not* adding this extra layer unless
  measurement proves the file read itself is material.

**Recommended cheapest-correct shape**: `grim hook run` reads exactly one
file (the dispatch table), does an in-memory lookup keyed on
`(client, event)` passed via CLI args, and on a miss writes nothing and
exits 0 — no config, no lock, no install-state, no OCI/network, no tokio
runtime construction, no regex compilation (matching is exact-key lookup
against pre-resolved entries, not per-invocation pattern compilation).

## Q7 — How to measure this honestly

**Tooling:**

- **Linux/macOS**: `hyperfine` for wall-clock distribution, with
  `--warmup N` to prime page/inode caches before recording (hyperfine's
  own docs distinguish this from cold-cache measurement, which needs an
  explicit `--prepare 'sync; echo 3 | sudo tee /proc/sys/vm/drop_caches'`
  or equivalent) — measure **both** conditions, not just warm.
  `perf stat` (Linux only) for syscall/cycle-level attribution when
  wall-clock alone doesn't explain a regression; `strace -c` to get a
  syscall-count/time breakdown for the "what dominates a cold Rust
  binary's startup" question in Q3, since no source found gave an
  authoritative isolated number for that.
- **Windows**: no Windows-native equivalent to hyperfine's shell-timing
  calibration was found in this research — `NOT DOCUMENTED`. `hyperfine`
  itself runs on Windows and can still time process launches, but its
  shell-overhead subtraction is calibrated against whatever shell is
  invoked (`cmd.exe`/PowerShell), which has different characteristics from
  Linux `sh`; the implementation task should verify hyperfine's Windows
  behavior directly rather than assume Linux methodology transfers.
  Windows Performance Recorder / `xperf` process-creation ETW tracing is
  the conventional deep-dive tool for CreateProcess-level attribution, but
  no source in this research validated it for this specific use case.
- **Statistics to report**: p50 and p99 (not mean — Q2's own primary
  source flags process-launch benchmarks as highly sensitive to
  background interference, which produces a long tail rather than a
  shifted mean), separately for cold and warm conditions, separately per
  platform (Linux, macOS, Windows-native, WSL2, and WSL2-with-workspace-
  on-a-Windows-mount as its own row per Q2's filesystem-boundary finding).

**Pitfalls, drawn directly from sources found:**

- **Cache warmth** — hyperfine's own guidance: benchmarks are "heavily
  influenced by whether the disk caches are warm or cold"; report both
  conditions explicitly rather than picking one.
- **Antivirus on Windows** — the bitsnbites.eu benchmark explicitly calls
  out that Windows process-launch timing is "very sensitive to background
  services such as Windows Defender and other antivirus software" —
  meaning a single Windows number is close to meaningless without stating
  what security software was active, and a real deployment will have it
  active. Measure with default-enabled Defender, not disabled, since
  that's the real user condition.
- **CI noise** — not directly sourced in this research, but implied by
  every "outlier detection" feature hyperfine ships (it flags statistical
  outliers from interference) — shared CI runners are noisy neighbors;
  the implementation task should either run on dedicated/pinned hardware
  or treat CI-measured numbers as directional only, with local
  measurement as the number that matters for design decisions.
- **WSL2's two distinct penalties must not be conflated** — process-spawn
  cost (fork/clone translation overhead, ~2-5 ms per Q2) and filesystem
  cross-boundary cost (9P protocol, ~6 ms per `stat()` in one report) are
  separate mechanisms; a benchmark run with the repo/dispatch-file on a
  Windows-mounted path will show a number dominated by the second effect,
  not the first, and must be labeled accordingly.
- **hyperfine's own shell-subtraction calibration** can itself introduce
  noise for very fast commands (<5 ms) — its docs recommend `-N` /
  `--shell=none` to skip shell-spawn subtraction entirely for exactly this
  regime, which is likely to be grim's regime on Linux/macOS.

---

## Sources

| URL | What it establishes | Fetched |
|---|---|---|
| https://www.bitsnbites.eu/benchmarking-os-primitives/ | Cross-platform process/thread creation and full-launch (fork+exec vs CreateProcess) relative cost ratios; Windows Defender/AV sensitivity note. `[unofficial: single-author benchmark, methodology described, not independently reproduced]` | 2026-08-14 |
| https://github.com/starship/starship/discussions/580 | Per-module timing breakdown example, `command_timeout` mitigation, tokio-async concurrent module execution, git-op caching within one render pass | 2026-08-14 |
| https://github.com/evilmartians/lefthook/wiki/Benchmark-lefthook-vs-pre-commit | Confirms lefthook publishes **no** concrete benchmark numbers despite the page title — feature/marketing prose only | 2026-08-14 |
| https://pnpm.io/blog/releases/11.0 | Confirms `pnpm server` (a persistent daemon) was removed in pnpm 11, with no stated reason in the release notes | 2026-08-14 |
| direnv.net / direnv.org (via search snippets, not directly fetched) | "Single static executable," stateless per-prompt existence check design, explicit non-daemon core with community caching add-ons for specific slow backends (Nix) `[unofficial: search-snippet sourced, not independently re-fetched in full]` | 2026-08-14 |
| GitHub search snippets: mise.jdx.dev "Comparison to asdf" and related | asdf shim overhead (~120 ms/command) vs mise's PATH-manipulation approach (~5-10 ms), 20x-200x improvement figure `[unofficial: project's own comparison docs, not independently re-benchmarked]` | 2026-08-14 |
| npm eslint_d page / project README (via search snippets) | Daemon reduces cold ESLint/Node invocation from ~700 ms to ~160 ms, to <50 ms bypassing the CLI wrapper via raw socket `[unofficial: project's own claims]` | 2026-08-14 |
| GitHub pre-commit/pre-commit-hooks#1069 (via search snippets) | ~50 ms per hook / ~600 ms total, attributed to Python interpreter + per-hook virtualenv overhead `[unofficial: self-reported GitHub issue]` | 2026-08-14 |
| git-scm.com/docs/githooks | Canonical git hook invocation contract: exec if executable, else `sh`-interpreted; no config/dispatch layer, no published performance figures | 2026-08-14 |
| docs.rs `regex` crate `PERFORMANCE.md` (via search snippets) | Regex compilation cost ranges from "a few dozen microseconds" (simple patterns) to ~44 ms (`\pL{100}`, Unicode-heavy); crate's own recommendation to amortize via `lazy_static`/`OnceLock` | 2026-08-14 |
| GitHub Locietta/xanmod-kernel-WSL2#8 and BuildStream buildstream#1217 (via search snippets) | Native Linux fork ~30 µs vs WSL2 simple-process fork ~2-5 ms (and up to ~50 ms for a heavier real-world fork); `lxss.sys` translates fork/clone to NT process creation `[unofficial: GitHub issue comments, self-reported, no shared harness]` | 2026-08-14 |
| GitHub microsoft/WSL#13846, vxlabs.com WSL2 I/O post (via search snippets) | WSL2 9P-protocol cross-filesystem cost is a **separate** mechanism from process-spawn cost — ~6 ms per `stat()` across the Windows-mount boundary in one report `[unofficial]` | 2026-08-14 |
| github.com/sharkdp/hyperfine (README, via search snippets) | Warmup-run guidance, cold-cache `--prepare` pattern, `-N`/`--shell=none` for sub-5ms commands, built-in statistical-outlier detection | 2026-08-14 |
| `/mnt/wsl/share/dev/grimoire/grimoire/src/context.rs` (read directly, commit `03e59b0`) | `Context::new()` is env-reads-only, no I/O; OCI access client is `OnceLock`-memoized and built lazily on first use, never on plain construction | 2026-08-14 |
| `/mnt/wsl/share/dev/grimoire/grimoire/src/config/project_config.rs` (read directly, commit `03e59b0`) | `walk_up_for_config`/`walk_up_from` perform the directory-ascent config discovery that is the actual "config discovery by walk-up" cost referenced in the trampoline research's F5 — a per-command step distinct from `Context::new()` | 2026-08-14 |
| `.agents/research/research_hooks_trampoline.md` (this repo) | F5 states the requirement this file answers; F2/D1 establish the `sync_config` convergence pass this file recommends as the dispatch-table writer | 2026-08-14 |

## Durable search terms

`lefthook benchmark vs pre-commit` · `starship prompt command_timeout git
status caching` · `mise vs asdf shim overhead benchmark` · `eslint_d daemon
socket latency` · `bitsnbites benchmarking os primitives fork exec
CreateProcess` · `WSL2 fork clone lxss.sys process creation overhead` ·
`WSL2 9P filesystem stat latency /mnt/c` · `regex crate PERFORMANCE.md
compile time lazy_static` · `tokio runtime construction cost` ·
`pnpm server removed daemon` · `direnv no daemon stateless design` ·
`hyperfine warmup cold cache methodology --shell=none` · `grim
walk_up_for_config Context::new`
