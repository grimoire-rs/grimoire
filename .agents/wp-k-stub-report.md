# WP-K — stub phase report

**Branch:** `hex/hooks-artifact-kind--wp4-k` (from feature tip `3772b76`)
**Phase:** Stub (public surface only). Bodies are `unimplemented!()` **only** on paths
`grim hook run` / `grim hook list` cannot reach — proven by execution below.
**Gates:** `cargo fmt` · `cargo clippy --locked --all-targets -- -D warnings` (clean) ·
`cargo check --all-targets` (clean) · `cargo test --bin grim` **2812 passed** ·
`task --force verify` green (**2812** unit, **1019** acceptance, **51** AI-config).
`.claude/tests/uv.lock` and `test/uv.lock` reverted after the gate.

---

## 1. The surface created

### `src/command/hook.rs` — module root

```rust
pub mod argv;  pub mod envelope;  pub mod list;
pub mod pipeline;  pub mod projector;  pub mod run;

pub struct HookArgs { pub command: HookCommand }        // clap Args
pub enum HookCommand { Run(run::RunArgs), List(list::ListArgs) }   // clap Subcommand
```

### `src/command/hook/argv.rs` — the untrusted-argv contract (B1, B3)

```rust
pub struct RunTarget<'a> { pub table: &'a Path, pub client: &'a str,
                           pub event: CanonicalEvent, pub root: &'a str }
pub enum ArgvRefusal { TableNotAbsolute, UnknownEvent, EmptyRoot, EmptyClient }
impl ArgvRefusal { pub fn reason(self) -> &'static str }          // + Display
pub fn validate(args: &RunArgs) -> Result<RunTarget<'_>, ArgvRefusal>;   // IMPLEMENTED
pub fn canonical_event(spelling: &str) -> Option<CanonicalEvent>;       // IMPLEMENTED
```

`ArgvRefusal` is deliberately **not** an error type: every variant is a refusal that exits **0**,
and modelling them as errors is how one would eventually acquire a non-zero exit code and start
denying tool calls on a fail-closed client.

### `src/command/hook/run.rs` — the runtime

```rust
pub struct RunArgs { pub client: String, pub event: String,
                     pub table: PathBuf, pub root: String }        // all four required
pub async fn run(args: &RunArgs) -> ExitCode;                      // IMPLEMENTED to the stub boundary
fn root_entry<'a>(table: &'a DispatchTable, token: &str) -> Option<&'a DispatchRoot>;  // IMPLEMENTED
fn audit_trail_path(table: &Path) -> Option<PathBuf>;              // IMPLEMENTED (see finding F-2)
async fn dispatch(target: &RunTarget<'_>, armed: &[&DispatchEntry]) -> ExitCode;  // unimplemented!, UNREACHABLE
fn client_admits(client: &str, entry: &DispatchEntry) -> bool;     // unimplemented!, UNREACHABLE (see F-1)
fn matches_tool(matcher: Option<&str>, tool: Option<&str>) -> bool; // unimplemented!, UNREACHABLE
```

`run` returns `ExitCode`, **not** `Result` — decision G and invariant I3 as a type. It cannot fail.

### `src/command/hook/envelope.rs` — C-002

```rust
pub const ENVELOPE_SCHEMA: u32 = HOOK_SCHEMA_VERSION;
pub const ENV_ALLOWLIST: [&str; 9];      // closed, non-secret-bearing (I6)
pub struct EnvelopeMeta<'a> { event, native_event, client, scope, hook, tier, cwd,
                              session_id, correlation_id }
pub enum EnvelopeError { RawNotAnObject }
pub struct ToolRef<'a> { pub name: &'a str, pub input: &'a [u8] }
pub fn build(meta: &EnvelopeMeta<'_>, raw: &[u8]) -> Result<Vec<u8>, EnvelopeError>;  // unimplemented!
pub fn tool_from_raw(raw: &[u8]) -> Option<ToolRef<'_>>;                              // unimplemented!
pub fn environment(meta: &EnvelopeMeta<'_>, payload_file: Option<&Path>) -> Vec<(String, String)>;  // unimplemented!
```

**`build` returns `Vec<u8>` and there is no `Serialize` impl on the envelope, and that is the
design rather than an omission.** C-002 requires `raw` byte-for-byte identical to the vendor
payload, never re-serialized through grim's serde. A `#[derive(Serialize)]` struct with
`raw: serde_json::Value` round-trips the payload through grim's parser and emitter, normalizing key
order, number formatting, escape forms and duplicate keys — so a hook whose job is to judge what
the client said would judge grim's paraphrase. The envelope is therefore **assembled**: grim's own
fields encoded with `serde_json`, the client's bytes spliced in verbatim. The module doc states that
adding a convenience `Serialize` impl later *is* the defect.

### `src/command/hook/projector.rs` — C-004 / C-021

```rust
pub const EVENT_ECHO_FIELD: &str = "hookSpecificOutput.hookEventName";
pub const EVENT_ECHO_CLIENTS: [&str; 2] = ["claude", "codex"];
pub enum ProjectionError { NoSurface{..}, Unpermitted{..}, Forbidden{..} }   // + Display + Error
pub fn permitted_fields(client: &str, event: CanonicalEvent) -> Option<BTreeSet<&'static str>>;  // IMPLEMENTED
pub fn forbidden_fields(client: &str, event: CanonicalEvent) -> Option<&'static [&'static str]>; // IMPLEMENTED
pub fn project(client, event, native_event, response) -> Result<serde_json::Value, ProjectionError>; // unimplemented!
```

**No second projection table.** `permitted_fields` is *derived* from the one
`RESPONSE_PROJECTION` row via `projection_for` — every verdict field, the reason companion, the
context target, the mutation target, plus the event echo. Two agreement tests ship at stub phase
(`every_shipped_pair_permits_its_own_verdict_and_reason`, `permitted_and_forbidden_never_overlap`).

### `src/command/hook/pipeline.rs` — C-003 + Decision O

```rust
pub enum Decision { Allow, Deny, Ask, None }   pub fn rank(self) -> u8   // + Display + Serialize
pub struct CanonicalResponse { decision, reason, context, user_message, stop, updated_input }
impl CanonicalResponse { pub fn no_opinion() -> Self }
pub struct TierPlan<'a> { pub mutators, pub gatekeepers, pub observers: Vec<&'a DispatchEntry> }
pub struct HookOutcome { pub hook: String, pub tier: HookTier, pub response: CanonicalResponse }
pub fn order<'a>(armed: &[&'a DispatchEntry]) -> TierPlan<'a>;   // IMPLEMENTED
pub fn aggregate(outcomes: &[HookOutcome]) -> Decision;          // IMPLEMENTED
pub async fn compose(plan: &TierPlan<'_>, raw: &[u8]) -> CanonicalResponse;  // unimplemented!
```

`order` and `aggregate` are implemented because they *are* Decision O parts 1, 3 and 4 and they are
pure — four tests pin them (`order_keeps_declaration_order_within_each_tier_o1`,
`deny_absorbs_every_other_verdict_o3`, `ask_outranks_allow_o4`, `an_observer_verdict_is_ignored`).
`TierPlan`'s two-field shape is part 2's enforcement: a gatekeeper is unreachable from the phase
that runs the mutators, so it cannot see pre-mutation input.

### `src/command/hook/list.rs` + `src/api/hook_report.rs` — S-015

```rust
pub struct ListArgs {}                                   // scope comes from the root flags
pub async fn run(ctx: &Context, args: &ListArgs) -> anyhow::Result<(HookListReport, ExitCode)>;
pub struct HookListEntry { artifact, id, tier, events, state: ArtifactStatus, arming: Vec<HookArming> }
pub struct HookListReport { pub items: Vec<HookListEntry> }   // Printable, `{"items": [...]}`
```

Plain: one 6-column table (Hook | Tier | Events | Client | State | Detail), one row per
`(entry, affected client)`. `state` / `cause` / `message` reuse WP-H's `ArtifactStatus` and
`HookArming` — a second vocabulary for the same facts is how `grim status` and `grim hook list`
would come to describe one hook differently.

### Wiring

- `src/command.rs` — `pub mod hook;`
- `src/api.rs` — `pub mod hook_report;` + re-export
- `src/main.rs` — `Command::Hook(HookArgs)`
- `src/app.rs` — **two** arms: an early `Command::Hook(HookCommand::Run(..))` return *before*
  `Context::new`, and the ordinary `List` arm after it (see F-3).
- `taskfile.yml` — `bench: ./taskfiles/bench.taskfile.yml`
- `taskfiles/bench.taskfile.yml` — `bench:hooks`, `bench:report`, `bench:fixtures`, `bench:clean`
- `.claude/rules/arch-threat-model.md` — `src/command/hook*` added to `paths:`, scope note rewritten
- `.claude/rules.md` — the "By auto-load path" row and the declared-overlap group cell, same commit

---

## 2. Reachability, with executed exit codes

`GRIM_LOG=debug ./target/release/grim …`, release build, WSL2.

| Invocation | Exit | Behaviour |
|---|---|---|
| `hook --help` | **0** | clap help |
| `hook run --help` | **0** | clap help |
| `hook list` | **0** | empty `items` report + one `debug` line |
| `hook run` (no flags) | **64** | clap usage error, before any hook code |
| `hook run … --table <abs match table>` | **0** | `WARN 1 hook(s) matched PreToolUse but hook dispatch is not implemented in this build; nothing ran` |
| `hook run … --event Stop` (no row) | **0** | `DEBUG no hooks armed at Stop …` |
| `hook run … --root 0000…ffff` (unknown token) | **0** | `DEBUG no hooks armed for the requested root; nothing ran` |
| `hook run … --table target/…/dispatch.json` (relative) | **0** | `WARN the --table path is not absolute, so it would resolve against the current directory; nothing was read and no hook ran` |
| `hook run … --event Bogus` | **0** | `WARN the --event value names no lifecycle event this grim understands` |
| `hook run --client '' …` | **0** | `WARN the --client value is empty, so it matches no known client` |
| `hook run … --table /nonexistent/dispatch.json` | **0** | `DEBUG no dispatch table at …; nothing is armed` |

W2 degrade paths, each with its distinguishable reason, all exit **0**:

| Table fixture | Exit | Reported reason |
|---|---|---|
| `{"schema": 999, …}` (newer grim, downgrade) | **0** | `UnknownSchema` |
| `{not json at all` | **0** | `Unparsable` |
| `[]` (JSON, not an object) | **0** | `Unparsable` |
| 2 MiB table (> `MAX_TABLE_BYTES`) | **0** | `Oversize` |
| row with a 300-byte matcher (> `MATCHER_MAX_BYTES`) | **0** | `RowRejected` |
| row with a relative `payload_dir` | **0** | `RowRejected` |

**No `unimplemented!()` is reachable.** `run` stops at the stub boundary with a `warn` line and
exit 0 before calling `dispatch`, `client_admits`, `matches_tool`, `compose`, `project`, `build`,
`tool_from_raw` or `environment`. That is the honest behaviour for a build that cannot dispatch,
and it is the same degrade direction every real failure takes. `grim hook list` returns an empty
report rather than panicking: nothing can install a hook until WP-J2 lands, so `[]` is factually
correct today (marked with a REMOVAL TRIGGER).

**64 is not a defect.** A missing required flag is clap's usage error raised during parse, before
any hook code exists, and the launcher's own `case` collapses every non-verdict exit code to 0
before a client sees it (`hook_launcher.rs`'s `VERDICT_EXIT_CODES` is empty for all three v1
clients). The only reader of that 64 is a human who typed the command.

---

## 3. The import test

One test module in `src/command/hook.rs`, three tests, all proven to **fail when violated**
(each was probed by introducing the violation, observing the failure, and reverting).

- **`the_runtime_imports_no_scope_no_config_no_data_root_c007`** — `include_str!` over the five
  runtime files (`argv.rs`, `envelope.rs`, `pipeline.rs`, `projector.rs`, `run.rs`; `list.rs` is
  the one declared exemption) and asserts none of them *names* four needles:

  | Needle | Why |
  |---|---|
  | `crate::config` | resolving config is scope resolution (C-007) |
  | `scope_resolution` | the same, by name |
  | `crate::context` | the carrier of config, scope and the data root |
  | `grim_home(` | `src/env.rs:26-34` returns the env value verbatim, with a **relative** fallback, and the CWD of a client-spawned run **is the workspace** (B1 · T3 · CWE-426) |

  Two deliberate spellings: `crate::context` rather than the bare type name, so a vendor field
  literal like `additionalContext` cannot trip it; and `grim_home(` as a **call**, so a renamed
  local cannot satisfy it. The scan strips comment lines before matching — that is what lets the
  runtime's own doc comments *explain* the forbidden symbols instead of only forbidding them.

  Probe: adding `fn _probe(_c: &crate::context::Context) {}` to `run.rs` →
  `hook/run.rs names \`crate::context\`, which the hook runtime may not …`. Adding
  `crate::env::grim_home()` → the `grim_home(` assertion fires. Both reverted.

- **`every_declared_runtime_module_is_checked`** — the guard on the guard: adding
  `pub mod spawn;` without listing it would leave the new file unscanned and the test green.
  Asserts the declared-module count equals the runtime list plus exactly one exemption.

- **`app_dispatches_the_runtime_before_it_builds_a_context_b1`** — asserts the first
  `hook::run::run(` in `src/app.rs` precedes the first `Context::new(`. Probe: deleting the early
  arm → `the \`grim hook run\` arm must be dispatched BEFORE the context is built …`. Reverted.

**No `ScopeResolver` seam**, per the plan: a production injection point with one real implementor
would add hot-path indirection to prove a compile-time truth and would *weaken* the guarantee — a
seam can be called, an absent import cannot.

---

## 4. The bench harness

`taskfiles/bench.taskfile.yml`, included from `taskfile.yml`. **Nothing gates on it** — no
threshold, and `task verify` does not reference it.

`hyperfine` is absent from this repository **and this machine** (D-13 re-verified:
`command -v hyperfine` → absent). `bench:hooks` refuses in **0.1 s** with an install hint rather
than reporting a fabricated number, and the build + fixtures are `cmds` rather than `deps`
precisely so the refusal precedes the 60-second release build (Taskfile runs `deps` *before*
preconditions).

Measured shapes: `no-match` (the **guard** path — paid on every tool call even when nothing is
armed, and it includes the extra `fork` the registration's dropped `exec` costs per B8) and
`match-and-dispatch`, each cold and warm, reported as **p50 and p99** computed from hyperfine's
JSON export by nearest-rank (no interpolation, so every figure is a measurement). The platform row
distinguishes **WSL2** from Linux by inspecting `platform.release()`.

The dev-doc and CI-image note lives in the taskfile's own header and `summary:` blocks;
`docs/src/**` and `AGENTS.md` are WP-M's files, so **WP-M owes the contributor-setup and CI-image
entries for `hyperfine`.**

---

## 5. Findings — wrong things in the plan or in already-merged code

### F-1 · **Block** — the dispatch table has no client dimension, so the runtime cannot honour a per-client decline, and a two-client hook would run twice

`DispatchRoot` (`src/install/hook_dispatch.rs:414-422`) is keyed by root token alone and its
`hooks` is a flat `Vec<DispatchEntry>`; `DispatchEntry` (`:373-410`) has **no `client` field**.
But `hook_registrar::desired_entries(vendor, …)` (`src/install/hook_registrar.rs:551-618`) is
**per vendor** — it filters `record.outputs` by `o.client == vendor.name()` and derives
`payload_dir` from *that vendor's* recorded output — while `converge_root` writes **wholesale per
root token** (`hook_dispatch.rs:661-723`). Those two shapes cannot both be right:

- **Union across vendors** ⇒ a hook armed for claude and codex produces **two rows** differing only
  in `payload_dir`, so one invocation runs the payload **twice**; and a hook grim `Declined` for
  one client (untranslatable matcher per C-025, or a tier that client cannot honour) still sits in
  that root's row set, so the declining client **would run code the user was told was not armed
  there**.
- **`converge_root` per vendor** ⇒ each vendor's write **wipes the previous vendor's rows** for
  that root, because the write is wholesale per key.

The `converge_root` call site does not exist yet (only doc references at `hook_registrar.rs:253`
and `:613`), so **this lands on WP-J2, which is running now.** The fix is an additive `client`
field on `DispatchEntry` (`#[serde(default)]`, Principle 9 safe) plus row selection on it. I did
not implement compensation in the runtime: re-deriving the decline from
`Vendor::hook_tier_support` and the matcher translation would be a second spelling of a
render-time decision, which is exactly the drift C-021 exists to prevent. Recorded as the doc
comment on `run::client_admits`.

### F-2 · **Warn** — the argv contract gives the runtime no audit-trail path, and the only derivation available reconstructs the authority `--table` was chosen to withhold

C-012 makes the runtime write an audit record; the ADR puts the trail at
`$GRIM_HOME/state/hook_audit.jsonl` (`adr_hooks_support.md:1617,1646`); the launcher passes
**only** `--table`, chosen over `--home` on the explicit least-authority ground that `--table`
"passes exactly the one path the runtime needs, where `--home` passes a directory from which the
runtime could derive the launcher, the payload trees, the root-key file and the content store"
(`hook_dispatch.rs:30-38`). To reach the trail the runtime must climb **two** levels from
`$GRIM_HOME/hooks/dispatch.json` to `$GRIM_HOME` — which is the `--home` authority, obtained
indirectly. The plan and the ADR never state where the *runtime's* trail lives.

Stubbed as `run::audit_trail_path(table)` doing the two-level climb (honouring the documented
location) with the gap recorded in its doc comment. Two better shapes, both outside this package:
a second baked argv element (`--audit '<abs>'`, at the cost of a registration string WP-I pins byte
for byte), or the trail **beside** the table inside the `0o700` hooks directory (one derivation
instead of two, at the cost of moving the ADR's documented location). **Orchestrator's call.**
Related: `AUDIT_FILE` is declared in `run.rs` because the runtime is its first writer; if the
install side needs it too, it belongs beside `AuditLog` in `src/hook/audit.rs`, not spelled twice.

### F-3 · **Warn** — "`grim hook run` never calls `env::grim_home()`" was **false for the process**, and the plan states the stronger claim

`Context::new` calls `env::grim_home()` unconditionally (`src/context.rs:169`), and `app::run`
built one `Context` for **every** command before any arm ran (`app.rs:27`, pre-change). So a
source-level import test on the hook module would have been green while the process still read an
attacker-choosable `GRIM_HOME` on every tool call — the value simply never *reached* the runtime.
Closed inside my own file set: the `Command::Hook(HookCommand::Run(..))` arm now returns **before**
`Context::new`, pinned by `app_dispatches_the_runtime_before_it_builds_a_context_b1`. Deleting the
early arm still compiles and still works, which is why the guard is a source-level test. Side
benefit: the environment is off the hot path of every tool call.

### F-4 · **Warn** — `src/oci/hook.rs`'s module-wide `#![allow(dead_code)]` names WP-K as its REMOVAL TRIGGER, and WP-K cannot discharge it — nor can any later WP

Measured by removing the attribute and running `cargo check --all-targets` with my stub in place.
Three items remain dead:

| Item | Why it stays dead |
|---|---|
| `RESERVED_POLICY_KEY` (`:127`) | nothing reads it — `policy` is parsed by field name, so the const is documentation-only |
| `HookSurface::CodegenModule` (`:753`) | **"No v1 implementor"** by design, documented as such at `:747-752` |
| `HookCommand::Argv` (`:759` region) | **"never constructed in v1"** by design (`hook_launcher.rs:82-83`) |

Two of the three are *deliberately* never constructed in v1, so the module-wide attribute is
undischargeable by construction, not merely not-yet-discharged. The honest fix is to replace it
with three per-item attributes (`#[expect(dead_code, reason = …)]`), at which point the removal
trigger becomes true for the rest of the module. **Not done here** — `src/oci/hook.rs` is merged
code my brief forbids editing. Reported, not fixed.

### F-5 · **Warn** — `RootToken` cannot be looked up, only scanned; correct, and worth stating

`RootToken` deliberately has no `Deserialize` and no production constructor from `&str` (removing
the transparent one is what closed a real hole — `hook_dispatch.rs:192-205`). Consequence: the
runtime, which holds the token only as an argv string, **cannot** do `table.roots.get(&token)`. It
compares against `RootToken::as_str` in a linear scan (`run::root_entry`). That is the right
trade — an unforgeable type beats an O(log n) lookup over a handful of keys — but it is a
consequence of a merged design decision that nothing in the plan records, and a later reader may
"optimize" it by re-adding a `&str` constructor. Documented at the call site.

### F-6 · **Suggest** — `EVENT_ECHO_FIELD` is the one projection fact `RESPONSE_PROJECTION` omits, and it wants a seventh column

`hookSpecificOutput.hookEventName` must echo the firing event on claude and codex, and
`RESPONSE_PROJECTION`'s own doc records it as a required const that is deliberately *not* a
per-row column. It is **not** derivable from a row: keying on "some field nested under
`hookSpecificOutput`" would wrongly include copilot's `PreToolUse`. So the projector carries one
const pair (`EVENT_ECHO_FIELD`, `EVENT_ECHO_CLIENTS`) — the smallest honest shape, but strictly
speaking a second (one-fact) table. The clean long-term form is a seventh `ProjectionRow` column
(additive, positions frozen), in `src/oci/hook.rs`. Left as a finding.

### F-7 · **Suggest** — for the Specify worker: S-004's test passes vacuously against this stub

"matcher does not match ⇒ exit 0, nothing spawned" is satisfied by a stub that spawns nothing at
all. The test must assert the *absence of a side effect* a spawned payload would produce (a marker
file), or it will be green for the wrong reason and stay green when matching regresses.

---

## 6. One-file overruns (reported, not silently widened)

Each checked against the plan (`grep` shows no other WP claims it), and each forced by a
convention that leaves no alternative — the plan's own "check the cell against the subsystem
rules first" box, and WP-H's precedent for `src/api/`.

| File | Why it could live nowhere else | Claimed by another wave-4 WP? |
|---|---|---|
| `src/main.rs` | `Command` (the clap subcommand enum) is defined there; `src/cli/**` has no enum to extend | No — WP-G's file, **merged** |
| `src/api/hook_report.rs` + one line in `src/api.rs` | `subsystem-cli-api.md` requires a `Printable` report to live in `src/api/{name}_report.rs`; WP-H took the identical overrun for C-017 | No |
| `taskfile.yml` (one `includes:` line) | a taskfile with no include is a dead file | No |

---

## 7. Left for Specify

- **C-002** — `raw` byte-for-byte (feed a payload with duplicate keys / unusual number formatting
  and assert the bytes survive); the env set is exactly `ENV_ALLOWLIST` and **no** variable carries
  tool input.
- **C-003 / C-004 / C-021** — the projector round trip per `(client, event)`; an unpermitted field
  is an **error**, never a silent drop; a forbidden field fails the render;
  `hook_tier_support` agrees with `RESPONSE_PROJECTION` (the C-021 agreement test).
- **C-007** — the import test ships (§3); add the argv cases: non-absolute `--table` ⇒ exit 0 and
  **nothing spawned**; unknown `--root` ⇒ exit 0 and nothing spawned (pair with WP-O's
  hostile-clone fixture).
- **C-009** — the matched path spawns without computing a digest.
- **C-011 / Decision O** — a gatekeeper never observes pre-mutation input; the mutator chain
  threads; `deny` suppresses the mutation entirely.
- **C-012 fail-closed** — the tier table: `observer`/`gatekeeper` ⇒ do not spawn, `NotSpawnedUnlogged`,
  exit 0, warn; `mutator` ⇒ spawn, **discard the rewrite**, `RewriteDiscardedUnlogged`, verdict stays
  `Some(Mutate)`, exit 0, warn. **Never a deny and never a non-zero exit.**
- **W2** — the six degrade fixtures in §2 exist and pass; add "malformed JSON ⇒ no panic" as an
  explicit assertion.
- **S-004…S-006, S-009, S-015, S-016** — note F-7 for S-004.

## 8. Left for Implement

`run::dispatch` (matcher → envelope → spawn with timeout → pipeline → projection → audit → exit 0),
`envelope::{build, tool_from_raw, environment}`, `projector::project`, `pipeline::compose`,
`run::matches_tool`, `run::client_admits` (**blocked on F-1**), `list::run`'s real declared set
(reusing `status.rs`'s `hook_arming` seam, which is private today — making it `pub` is a
`src/command/status.rs` edit). Every `#[expect(dead_code)]` in this package carries its REMOVAL
TRIGGER in the reason string; the four items with test readers already use
`#[cfg_attr(not(test), expect(…))]`, the form the plan's box prescribes.
