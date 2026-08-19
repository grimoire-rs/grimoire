# WP-I — Stub phase report (dispatch table, launcher, `sync_config` convergence)

Branch `hex/hooks-artifact-kind--wp-i`, worktree `.agents/worktrees/wp-i`, based on `9c82115`.
**Uncommitted, not pushed.** Gates: `cargo check --all-targets`, `cargo clippy --locked
--all-targets -- -D warnings`, `cargo test --bin grim` (2689 passed, 0 failed), `cargo fmt --check`
— all clean.

---

## 1. Defects found in the plan, the contracts, and already-merged code

Ordered by how much later work they would cost. Items 1–4 are **stale text in merged code that
contradicts the amended contracts**; 5–7 are **structural gaps the contracts do not name**; 8–10 are
**scope/ownership facts the plan gets wrong**.

### D-I-1 · Block · `Vendor::hook_registration`'s doc carries the SUPERSEDED command string

`src/install/vendor.rs:829-831` documents the registration "byte for byte" as:

```text
L='<launcher>'; [ -x "$L" ] || exit 0; exec "$L" run --client <client> --event <Event> --root <global|'<abs>'>
```

That is the **pre-audit** form on four of the five counts WP-P0 changed: no `[ -f "$L" ]` (B8), still
`exec` (B8), no `--table` (B1), `--root global|<abs>` instead of an opaque token (B3), and no
`s=$?`/`case` (B8). Only the single-quoted assignment (B2) survived. The four supporting bullets at
`vendor.rs:847-863` repeat the pre-B8 guard rationale ("The guard tests the launcher…" with no `-f`
clause).

WP-F merged **after** WP-P0's fold landed in the plan, so this is not a sequencing artifact — the
amendment simply was not propagated into the one doc comment a future implementer of
`hook_registration` will read as authoritative. It is a doc comment, so it compiles and no test
catches it; the next reader implements the superseded string. `vendor.rs` is outside my declared file
set, so I could not fix it.

### D-I-2 · Block · `HookRoot<'a>` is the pre-B3 type, and it is the type `hook_registration` takes

`src/install/vendor.rs:93-104`:

```rust
pub enum HookRoot<'a> { Global, Workspace(&'a Path) }
```

Those are exactly the two values B3 forbids from reaching the argv: `--root global` is a fixed
literal and `--root <abs workspace>` is usually guessable. Its doc argues both are safe because they
are "grim-chosen, never derived from client-supplied data" — which is the property WP-P0 attacked and
found **sound but insufficient**: grim is not the only writer of client hook configs, so the key must
be *unforgeable*, not merely grim-chosen. `hook_registration`'s signature takes `root: HookRoot<'_>`,
so as merged the assembly site cannot express B3 at all.

My `hook_dispatch::RootScope` is deliberately the same shape under a different name and a different
job: it is the **semantic** root grim reasons about, and `hook_dispatch::root_token` maps it to the
opaque `RootToken` that reaches the wire. Whoever implements `hook_registration` must take a
`&RootToken`, and `HookRoot` should be deleted rather than kept as a second spelling of `RootScope` —
two types for one concept, one of which is the forbidden wire form, is how B3 gets silently undone.

### D-I-3 · Warn · `HookCommand`'s doc still says Claude uses exec-form argv

`src/oci/hook.rs:527-532`: *"exec form removes shell quoting **and** shell expansion from the client
boundary, which is why claude's project-scope registration is safe to carry an absolute launcher
path"*, and it states the guard as `[ -x "$L" ] || exit 0`. WP-B § 6.1 refuted the premise by
execution — Claude Code 2.1.233 has no argv array, its `command` string is run by `/bin/sh` with full
expansion, and the safety comes from the absolute literal in a non-committed file, never from the
absence of a shell. The plan corrected this in three places; `hook.rs` was not one of them.
`HookCommand::Argv` is now never constructed in v1, which the doc does not say.

### D-I-4 · Warn · `HookRegistration`'s doc carries the exemption B2 identified as the hole

`src/oci/hook.rs:546-548`: *"the command is assembled from grim's own literals plus the absolute
launcher path grim resolved at install time"*. C-018b was **widened** by B2 precisely because the
resolved launcher path is *not* grim-owned — it is `env::grim_home()`-derived, returned verbatim with
no absoluteness check — and that exemption is what let a `$GRIM_HOME` containing `$(…)` run under a
double-quoted assignment. The sentence now states the pre-widening contract.

### D-I-5 · Block · there is no `Vendor::hook_config_path` seam

`Vendor::hook_surface()` answers the *shape* (`SpliceConfig` / `OwnFile`); **nothing answers the
path**. The three vendor doc comments name the files in prose (`~/.claude/settings.json`,
`$CODEX_HOME/hooks.json`, `~/.copilot/hooks/grim.json`) but there is no analogue of
`mcp_config_path`, so a registrar holding `&dyn Vendor` cannot ask a client where to write.

Worked around inside my file set: each vendor's own `sync_config` resolves its path with a private
`hook_config_path(workspace, scope) -> Option<PathBuf>` and passes it to
`hook_registrar::sync_for_state`. That keeps per-client path knowledge in the per-client file, exactly
as `mcp_config_path` does, and avoids the alternative — a `match vendor.name()` switch in a shared
module, which is the silent-drift shape D-1 is about. **Owed:** promote it to the trait when
`vendor.rs` is next open (WP-F implement or WP-J2), because a generic consumer needs it:
`grim status`'s `not-armed` probe and `expected_outputs` both have to locate a registration without
knowing which client they hold.

### D-I-6 · Block · `Vendor::sync_config`'s signature cannot carry C-017's refusal

`sync_config` returns `io::Result<()>`, and **all six callers log an `Err` as a warning**
(`installer.rs:395`, `uninstall.rs:136`, `update.rs:294`, `tui/app.rs:2465,2540,3557`). So a refusal
expressed as `Err` collapses four distinguishable policy causes into one opaque I/O line, and
`grim status` gets nothing structured. C-017 requires the opposite: a distinguishing message per
cause, plus a `not-armed` state.

Resolved without touching `vendor.rs`: the refusal is a **return value**
(`HookSync::NotArmed(ArmRefusal)`) of `hook_registrar::sync_for_state`, `Err` stays for genuine I/O,
and WP-H reads a separate read-only `hook_registrar::arming_refusal(...)` for the status path. WP-H
must be told this: the seam it wants is `arming_refusal`, not `sync_config`.

### D-I-7 · Warn · C-017 cause 4 is not reportable by `grim status`, as specified

The plan requires all four refusal causes to be both a refusal (WP-I) **and** a reported state
(WP-H). Causes 1–3 are pure functions of `$GRIM_HOME`, the workspace and the resolved paths, so
`grim status` re-derives them identically with no record — that is what makes them reportable at all
under Decision L. **Cause 4 (`DispatchLocked`) is write-time-only and transient**: an install that
reported `not-armed` because another install held the lock will read as *armed* at the next
`grim status`, and nothing can close that without recording the failure, which Decision L forbids.
Documented on `arming_refusal`; the honest surface is the install-time warning plus "the next
`grim install` converges". WP-H must not present the status answer as authoritative for cause 4.

### D-I-8 · Warn · B8's `case` allowlist is **empty** for all three v1 clients

B8 writes the last line as `case "$s" in 0) exit 0 ;; <grim's own verdict codes for this client>)
exit "$s" ;; *) exit 0 ;; esac`, and the matrix row "launcher returns a deliberate verdict `exit 2`
→ 2 (preserved)" reads as though a code must survive. C-004's `RESPONSE_PROJECTION` says otherwise:
**every** verdict field on **all twelve** v1 `(client, event)` rows is a JSON field on stdout
(`decision`, `hookSpecificOutput.permissionDecision`) — no client's projected blocking convention is
an exit code. With decision G ("the launcher never signals failure through its exit code"), grim's
exit-code verdict vocabulary is therefore empty, and collapsing every code to 0 preserves grim's
verdicts intact.

Encoded as `VERDICT_EXIT_CODES` with all three clients present-and-**empty** (an absent client would
be indistinguishable from an unconsidered one) and a doc comment explaining why the `case` shape is
still emitted: one string shape, one code path, S3-pinnable, and the next client whose only verdict
channel *is* an exit code — Claude's own documented `exit 2` form, which grim does not project —
needs one arm added and nothing else.

### D-I-9 · Warn · `posix_single_quote` now exists twice, and the copy with no caller is WP-F's

WP-F implemented `posix_single_quote` as a **private** fn in `vendor.rs:503` under
`#[expect(dead_code)]`, reasoning that "this quoting *is* the C-018b argument". I reached the same
conclusion for the generator and implemented it in `hook_launcher.rs` — where the caller is. Private
visibility means I could not reuse WP-F's, so there are two copies of a security-relevant function.
**One must go, and it is WP-F's:** once `hook_registration` composes
`hook_launcher::registered_command`, the vendor.rs copy has no caller. Delete it in the same commit
that implements `hook_registration`, or the next reviewer has to decide which of two quoting
implementations is authoritative.

### D-I-10 · Warn · two dependency facts the plan does not account for

- **The root-key RNG.** B3's HMAC needs 32 bytes from an OS CSPRNG. No direct dependency provides
  one. `getrandom` is **already in `Cargo.lock`** transitively — at **0.2.17 and 0.3.4** — so a direct
  `getrandom = "0.3"` reuses the locked 0.3.4 and costs zero net-new crates. `Cargo.toml` was outside
  this WP's file set at Stub, so `machine_key`'s body is stubbed; **it is added to the file set for
  Implement** (owner decision). Do **not** substitute `fastrand`, a clock or a pid: a guessable key is
  B3 with extra steps.
- **HMAC itself.** `hmac` is **not** in `Cargo.lock`. **Owner-approved: add `hmac = "0.12"`** — the
  canonical pairing with the locked `sha2` **0.10.9** (both RustCrypto, both on `digest` 0.10), so the
  trust delta is near zero. **Do not hand-roll RFC 2104.** A prefix-keyed `SHA256(key || root)` is not
  an acceptable option either (length extension).
  *(Corrected 2026-08-17: the Stub report first read `sha2` as the 0.11 pre-release and `getrandom` as
  0.4.3. Both were wrong — the lock says `sha2` 0.10.9 and `getrandom` 0.2.17/0.3.4 — so the
  digest-compatibility concern that motivated the hand-rolled fallback does not exist.)*
- **`ExperimentalOptions::hooks_enabled`** (`src/config/declaration.rs:229`) carries
  `#[expect(dead_code)]`. Its first caller is this registrar, so the Implement commit must delete
  that attribute — in WP-E's file, outside my set.

---

## 2. The two owed choices, settled

Recorded here and in the module docs so the Progress Log can pick them up verbatim.

| Choice | Settled | Why |
|---|---|---|
| **B1 — `--table '<abs>'` vs `--home '<abs>'`** | **`--table`** | Least authority. `--table` passes exactly the one path the runtime needs; `--home` passes a **directory** from which the runtime could derive the launcher, the payload trees, the root-key file and the content store. Every such derivation is a second runtime input smuggled behind one argv value, and C-007's "the table is the sole runtime input" stops being checkable by reading the argv. |
| **B3 — 128 random bits vs HMAC of the root** | **HMAC-SHA256 of the root under a machine-local key**, hex, truncated to 128 bits; key at `$GRIM_HOME/hooks/root-key`, mode `0o600` inside a `0o700` dir | The token must be **derivable on demand**, because re-materialization has to find its own entry. Stored randomness needs a path→token map — a second piece of mutable state whose loss or partial write strands a workspace's hooks with no way to name the orphaned record. `HMAC(key, root)` needs no map and is stable across re-installs by construction. |

---

## 3. What landed

Three new modules, wired into `src/install.rs`, plus a `sync_config` impl and a private
`hook_config_path` on each of the three v1 vendors. Public surface, doc comments carrying the
contract, `unimplemented!()` bodies — except where a body **is** the argument (see below).

### `src/install/hook_dispatch.rs` — C-006

`HOOKS_DIR` · `DISPATCH_FILE` · `ROOT_KEY_FILE` · `DISPATCH_SCHEMA` · `MAX_TABLE_BYTES` ·
`HOOKS_DIR_MODE` (`0o700`, W3) · `TABLE_MODE` (`0o600`, W3) · `hooks_dir` · `dispatch_path` ·
`root_key_path` · `RootScope{Global,Workspace}` + `display()` · `RootToken` (newtype) ·
`root_token()` · `machine_key()` · `DispatchEntry` (incl. `resolved_digest`, W4 — documented
provenance-only, never a gate) · `DispatchRoot` (token → readable root, diagnostics only) ·
`DispatchTable{schema,roots}` + `empty()` · `DispatchDegrade` (the five W2 degrade reasons) ·
`read_table() -> (DispatchTable, Option<DispatchDegrade>)` — **never `Err`, never panics** ·
`DispatchWrite` · `DispatchError{Locked,Io}` · `converge_root()` (wholesale per key, under
`AdvisoryFileLock`, through `atomic_write`).

**One parser, and it lives here.** WP-K's runtime calls `read_table` rather than deserializing the
file itself — two readers of one format drift, and the drift direction is "the runtime honours a row
the writer would not have written", which is the C-021 lesson applied to the table. `read_table`
takes a `&Path` (already absolute), which is also what makes WP-K's "never calls `env::grim_home()`"
import test satisfiable.

### `src/install/hook_launcher.rs` — C-008

`LAUNCHER_DIR` · `LAUNCHER_FILE` · `LAUNCHER_MODE` (`0o755`, separate `chmod` — `atomic_write` caps
at `0o644`) · `launcher_dir` · `launcher_path` · `VERDICT_EXIT_CODES` + `verdict_exit_codes()` (see
D-I-8) · **`posix_single_quote` (implemented)** · `powershell_single_quote` (implemented) ·
`CommandRefusal::ControlCharacterInPath` · `CommandSpec{launcher,table,client,event,root}` ·
`registered_command()` · `registered_command_powershell()` (codex `commandWindows` / copilot
`powershell`; runtime-unverified on Windows, watchlisted) · `shim_body()` · `LauncherWrite` ·
`LauncherError` · `generate()`.

`CommandSpec` is **C-018b as a type**: its fields are the closed set of values that appear in the
generated line, so "no value grim did not itself choose is interpolated" is checkable by reading a
struct declaration instead of auditing a format string. `matcher`, `hook.id`, the artifact name and
every vendor override are absent by construction.

The two quoting functions are implemented rather than stubbed for WP-F's own stated reason: the
quoting *is* the C-018b argument, and a body-less version leaves the constraint unexpressed.

**W9 taken explicitly** (the plan permits it "if it does so explicitly"): the shim resolves grim by
recorded absolute path and **exits 0** when that path is gone — no `$PATH` fallback. A5's fallback
would let a poisoned `$PATH` from the *client's* inherited environment choose the binary the trusted
shim executes, which is CWE-426 reintroduced inside the one file the design treats as trusted.
Re-running `grim install` regenerates the shim, so the fallback bought nothing a supported command
does not. Note `exec` **is** used inside the shim (correctly: its job is to *become* grim, and a
failed `exec` lands in the registration's own remap) and **not** in the registration — the contrast
is deliberate and documented at both sites.

### `src/install/hook_registrar.rs` — Decision L, C-017

`CLAUDE_LOCAL_SETTINGS` · `GIT_EXCLUDE_RELATIVE` · `ArmRefusal` (causes 1–4, one distinguishing
`reason()` each) · `HookSync{NoHooks,Unchanged,Armed(n),Disarmed,NotArmed(ArmRefusal)}` ·
`sync_for_state()` (the entry point, ordered contract in the doc) · `has_hook_record()`
(**implemented**) · `arming_refusal()` (read-only, WP-H's seam) · `validate_grim_home()` (causes 1–2)
· `desired_entries()` · `root_scope_for()` · `ExcludeOutcome` (7 outcomes) ·
`ensure_settings_local_excluded()` · `drop_settings_local_exclude()`.

The module doc states the enumerate-and-reap obligation as non-optional and names
`owned_nested_handlers` explicitly, records WP-D's N-3 limit (per-event `member` enumeration misses an
entry under an event key this binary does not project), and states the marker-re-assertion obligation
(idempotent on every `grim install`, because it is unverified whether Claude preserves an unknown
member when the client rewrites the `hooks` block).

The git-exclude pair is documented as **best-effort, never a gate**, with the I3 reasoning inline:
every non-`Added` outcome still arms, and `AlreadyTracked` is the outcome most worth surfacing
because it means the user's own arming *will* show up in `git status`.

### The vendors

Each `sync_config` resolves its own surface path and delegates. Doc comments carry the
per-client asymmetries: claude is the only project-scope registration and the only splice surface;
codex is `OwnFile` global-only with a human `/hooks` trust step that is **not** an `ArmRefusal`
(grim converged correctly — that third state is WP-H's); copilot is `OwnFile` global-only, the only
fail-closed client, PascalCase-mandatory, and must never use the exec-form field.

---

## 4. One regression I introduced and fixed — worth reading before Implement

Wiring `sync_config` on three vendors gave `hook_registrar::sync_for_state` a **live** caller on
every install, update, uninstall and TUI action, so an `unimplemented!()` body broke **19 existing
tests** (`tui::app::tests::perform_*`, installer tests). Fixed by implementing the real fast path
first: no hook record for this client ⇒ `HookSync::NoHooks`, before any read. That is the correct
production shape (`sync_config` runs for every client on every action and must cost nothing in the
common case) and it is the `want` computation of `opencode_config::sync_for_state`, one kind over.

**The guard is not yet the whole no-op condition, and the code says so.** Convergence must also run
when no hook is recorded but a grim-owned registration still exists — the reap-after-uninstall case,
where the record naming the group has already left state and only `owned_nested_handlers` can find
what to remove. That branch needs the enumeration, so it lands with the body. The Implement commit
must extend the guard to `!has_hook_record(..) && !owns_anything(..)` **in the same commit as the
body**, or a registration stays armed in a user-owned file forever.

### Is `grim uninstall` leaving a registration armed **today**? No — verified by source

Not a wave-3 blocker; a **WP-J2-gated obligation**. The chain, three links, each read rather than
assumed:

1. **No hook record can reach `state.json` through any shipped seam.** `locate_canonical`
   (`installer.rs:2446` and `:2466`) refuses `ArtifactKind::Hook` with
   `oci::hook::unsupported_kind()` — DataError/65 — *before* any record is written. The merged code
   states the resulting invariant itself, at `client_target.rs:310-312`: *"no hook record can be
   written while every install seam refuses the kind."*
2. **So `has_hook_record` is always `false`**, `sync_for_state` returns `NoHooks`, and nothing is
   written — which is why all 2689 tests pass.
3. **So there is no registration to strand.** `grim uninstall` cannot leave one armed, because
   nothing can arm one.

The path becomes reachable in **exactly** the commit that first lets `install_one` produce a hook
record — WP-J2, wave 4. That is the ordering constraint worth pinning: the guard extension must land
in or before that commit, not merely "during Implement".

**One caveat that is not mine alone.** A **hand-edited** `state.json` carrying `"kind": "hook"` does
deserialize — `ArtifactKind` is `#[serde(rename_all = "lowercase")]` and `InstallState`'s
`try_from = "RawInstallRecord"` validates only the `pinned` XOR `path`/`hash` pair, no kind filter —
so that input reaches `has_hook_record == true` and then my `unimplemented!()`. It is a reachable
panic, and the branch **already carries the same class in three merged sites on the same input**:
`client_target.rs:312`, `client_target.rs:364`, `path_anchor.rs:721`, each an `unreachable!()` whose
comment asserts the invariant from link 1. All four should become refusals via
`oci::hook::unsupported_kind()` — the pattern `installer.rs:2446` already uses, and A-3's rule applied
consistently — rather than panics: mine in my Implement pass, the other three in WP-J1/WP-J2.
Attacker-wise this is **N2/N4** (the user's own file, at user privilege), so it is an I3 robustness
item, not a security finding. **Not executed** — a probe writing a hand-edited state file was declined
by the sandbox, so this link is source-derived only.

---

## 5. Deferred, carried forward verbatim

- **W3** (T5 · I1, I5) — shared `$GRIM_HOME` puts the arming authority in another trust domain. The
  two mode constants are declared (`HOOKS_DIR_MODE` `0o700`, `TABLE_MODE` `0o600`) and
  `atomic_write`'s `mode & 0o644` preservation means a `0o600` file *stays* `0o600`, so the tighter
  mode is implementable with the shipped primitive. The **refusal** on a group-/other-writable table
  or launcher (C-017 cause 5) is not implemented; the doc half is WP-G's/WP-M's.
- **S1** — verify the generated launcher at install time (regular file + exec bit + resolvable
  interpreter, else `not-armed`). `LAUNCHER_MODE`'s doc records why it matters: `atomic_write` caps at
  `0o644`, so `0o755` is a separate `chmod`, and a silent failure there means the hook never fires.
- **S2** — keep the guard's single-line stderr diagnostic (silencing it hides a real failure from the
  vendor's log); grim's own `not-armed` is the durable signal.
- **S3** — pin all four registered command strings byte-for-byte as golden fixtures. Strongly
  recommended given D-I-1: a stale doc comment is exactly how the superseded string comes back.
- **WP-B § 4's `$SHELL` risk** — codex runs hooks through `$SHELL -lc`, a *login* shell, so a `fish`
  or `nushell` user cannot execute `L='…';` at all. Recorded in the `hook_launcher` module doc;
  belongs in `vendor-capability-watchlist.md` (WP-M) as a "hook silently never fires" class.

## 6. Not reachable from this file set

- D-I-1, D-I-2, D-I-3, D-I-4 (`src/install/vendor.rs`, `src/oci/hook.rs`) — stale contract text in
  merged code.
- D-I-5's trait promotion (`vendor.rs`).
- D-I-9's duplicate deletion (`vendor.rs`).
- D-I-10's dependency additions (`Cargo.toml`) and the `hooks_enabled` attribute removal
  (`src/config/declaration.rs`).
