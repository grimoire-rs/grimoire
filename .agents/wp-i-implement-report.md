# WP-I — Implement phase report (dispatch table, launcher, `sync_config` convergence)

Branch `hex/hooks-artifact-kind--impl-i`, worktree `.agents/worktrees/impl-i`, based on `0387aa7`.
**Uncommitted, not pushed.**

Gates, all clean: `cargo check --all-targets`, `cargo clippy --locked --all-targets -- -D warnings`,
`cargo fmt --check`, `cargo test --bin grim` (**2729 passed, 0 failed** — 2689 pre-existing + 40 new),
and the full **`task --force verify` exited 0** (clippy, fmt, 51 AI-config tests, `catalog:verify`,
`shell:verify`, 2729 nextest unit tests, **1019 acceptance tests passed**).

---

## 1. Wrong in the merged code or the contracts

### D-I-11 · Block · `hmac = "0.12"` **cannot compile** against this repo's `sha2` — it must be `0.13`

The approval, and the Stub report's own 2026-08-17 correction it rests on, both read `sha2` as
**0.10.9**. `Cargo.toml:35` declares `sha2 = "0.11"`; 0.10.9 is in `Cargo.lock` only **transitively**,
so `use sha2::Sha256` resolves to **0.11.0 / `digest` 0.11.3**. `hmac` 0.12 is the **`digest` 0.10**
line — `Hmac<Sha256>` would fail to typecheck across the two `digest` majors, and there is no way to
reach the transitive 0.10 `sha2` from crate code.

**Implemented `hmac = "0.13"`** (verified: its manifest requires `digest = "0.11.2"` and its own
dev-deps pin `sha2 = "0.11"`). The approval's *intent* is honoured exactly — RustCrypto, same
`digest` as the `sha2` already present, no hand-rolled RFC 2104, no prefix-keyed SHA-256 — only the
version number moves, because 0.12 does not satisfy that intent against this tree.

Lock delta is **additive only, 3 packages**: `hmac 0.13.0`, plus its constant-time helpers `cmov
0.5.4` and `ctutils 0.4.2`. `getrandom = "0.3"` was added as instructed and resolved to the
already-locked **0.3.4** with **zero** new packages.

> ⚠ **Resolve online, not `--offline`.** A first attempt with `cargo check --offline` produced a lock
> with **46 insertions / 18 deletions** because it silently *downgraded* `js-sys`, `wasm-bindgen`,
> `web-sys` and `wasi` to whatever the local cache held. Reverted and re-resolved online: 27
> insertions, 0 deletions, no version moved.

### D-I-12 · Block · `vendor::RootToken<'a>` is **unconstructible outside `vendor.rs`**

`src/install/vendor.rs:124` is `pub struct RootToken<'a>(&'a str);` — private field, **no
constructor, no `From`, no `new`** — and `Vendor::hook_registration`'s signature takes it by value.
Its own doc says "a token is constructed by the registrar's derivation", but the registrar is a
different module, so **nothing outside `vendor.rs` can call `hook_registration` at all**. WP-F's
implement pass must add a constructor (or take `&hook_dispatch::RootToken`), and D-I-2's point stands
harder than at Stub: there are now **two** `RootToken` types, and the `vendor.rs` one is the
uninhabitable half.

Unblocked-by-luck only: `hook_registration`'s body is still `unimplemented!()`, so nothing wanted to
call it this pass.

### D-I-13 · Warn · my own module doc had dropped the `0)` arm from the `case`

`hook_launcher.rs`'s "byte for byte" block read `case "$s" in *) exit 0 ;; esac`;
`Vendor::hook_registration` (authoritative) reads
`case "$s" in 0) exit 0 ;; <codes>) exit "$s" ;; *) exit 0 ;; esac`. Fixed, with a note recording the
divergence — this is precisely the D-I-1 failure mode reproduced inside the file that was created to
fix it. **The generated string follows `vendor.rs`.**

### D-I-14 · Warn · `desired_entries`' stub signature was unimplementable

`(state, workspace, scope, trust)` cannot produce a payload directory. A `ClientOutput::target` is an
`AnchoredPath`, so resolving it needs `AnchorRoots` — which `subsystem-file-structure.md` says is
resolved once, at scope-resolution time, and may not be re-derived from ambient env — and a hook's
payload is materialized **per client**, so "the payload directory" is undefined until a client is
named. Now `(vendor, state, roots, trust)`; `scope` was redundant (the state file is already
scope-specific) and `workspace` was only ever reachable *through* the roots. It is a private fn, so
this is a local correction; **WP-J2 must call the new shape.**

### D-I-15 · Warn · four cross-WP stubs make the convergence body unwritable this pass — see § 3

`json_splice::{owned_nested_handlers,upsert_nested_handler,remove_nested_handler,nested_handler_value}`
(WP-D), `Vendor::hook_registration` (WP-F), `HookManifest::from_toml_str` (WP-A) and WP-G's trust
predicate are all still `unimplemented!()`. Every one of them sits **inside** step 3–5 of
`sync_for_state`'s ordered contract, so a "real" convergence body would panic through somebody
else's stub. What landed instead is § 3.

### D-I-17 · Block, **redirected** · WP-G's `persist_grant` lock obligation lands on WP-J2/WP-K, not on WP-I — and it is on the *contract*, not just the hand-off

The substance is right and I am not disputing it: `persist_grant` is a read-modify-write of the
**global** `grimoire.toml` whose write seam re-serializes the whole file, so two concurrent grants
are last-writer-wins on **every declaration in that file**, not merely on the grant. That is data
loss in a hand-maintained file, and the project-scope lock a `grim install` holds guards a
different file and does not cover it.

**But WP-I has no call site to wrap, and cannot acquire one.** Three checks:

1. `grep -rn persist_grant src/` over my base (`0387aa7`) finds the definition at `trust.rs:589`
   and **zero callers**. Wrapping a call that does not exist is not implementable.
2. Every function in the chain — `arming`, `prompt_for_registry`, `persist_grant`,
   `interactivity` — is still `unimplemented!()`, so a call added here would panic through WP-G's
   stub, not lock anything.
3. **The designation itself is the defect.** `persist_grant`'s doc says "called from the hook
   arming path in `sync_config` (WP-I)", but `Vendor::sync_config` runs **once per client per
   command** — three v1 clients × install / update / uninstall / every TUI action. Prompting there
   prompts up to three times for one consent and persists up to three times, which contradicts
   C-023's one-time prompt *and* WP-G's own `Arming::ConsentRequired` doc ("kept out of `arming`
   itself so the decision stays pure and the prompt stays in exactly one place"). My trust seam is
   a **pure predicate** (`&dyn Fn(&LockedSource) -> bool`) for exactly that reason. The prompt and
   the grant belong at the command boundary, **above** the per-client loop — WP-J2's
   `installer.rs` or WP-K.

Recorded, not dropped: `desired_entries`' doc now states that `trust` must be pure, that the grant
belongs above this seam, and that when it runs there it must take
`command::scope_resolution::lockable_path` → `lock::file_lock::ConfigFileLock::try_acquire`,
**including the trap that the project-scope install lock does not cover the global config**. That
puts the obligation in the file the actual caller will be reading when it wires the predicate.

The two FYIs need nothing from me and were checked rather than assumed: `registry_resolve.rs` is
untouched (not in my diff), and my three modules contain **zero** loopback / `plain_http_hosts` /
`localhost` / `127.0.0.1` references, so there is no de-duplication for me to get wrong.

### D-I-16 · Warn · `HOOK_MARKER_VALUE`'s `expect(dead_code)` had to go, in `vendor.rs`

Using the marker (rather than duplicating the literal) made `vendor.rs:239`'s attribute unfulfilled,
which fails `-D warnings`. **Four lines deleted in a file outside my set**, mechanically, and the
attribute's own text names this as the trigger ("delete when the first splice writer lands — an
unused marker means nothing claims ownership of what grim wrote"). The alternative was a second copy
of the marker string, which is the drift shape D-1 is about.

---

## 2. The reachable panic is now a refusal (mandate 4)

`sync_for_state` is the **one** site, and it is closed. A hand-edited `state.json` carrying
`"kind": "hook"` deserializes (`InstallState` validates the `pinned` XOR `path`/`hash` pair, never the
kind) and previously reached `unimplemented!()` → exit 101, no `classify_error`, no JSON error
document, user blocked — the inversion of I3. It now returns
`io::Error::other(oci::hook::unsupported_kind())`, following `installer.rs:2446`, so every one of the
six `sync_config` callers logs it as today's warn-only sync failure and the primary command still
succeeds. Covered by `a_grim_owned_registration_with_no_record_no_longer_reads_as_a_no_op`.

`client_target.rs` and `path_anchor.rs` untouched, as instructed.

## 3. The no-op guard, widened now (mandate 5)

`!has_hook_record(..) && !owns_anything(..)`, landed **ahead of** WP-J2 rather than with it.

`owns_anything` is a **deliberate over-approximation with a stated direction**: `OwnFile` clients
(codex, copilot) are answered exactly — grim owns the path, so `is_file()` is the answer — and
`SpliceConfig` (claude) is probed as "`HOOK_MARKER_VALUE` appears in the file's bytes", because the
precise answer is WP-D's unimplemented `owned_nested_handlers`. The probe can say `true` with nothing
marked (one wasted convergence pass) and can **never** say `false` when a marked element exists (a
marked element cannot exist without its marker in the bytes). False-positive-only is the correct
direction; a false negative would strand an armed registration forever. Three tests pin all three
branches, including the unmarked-user-config case.

## 4. What landed, per module

**`hook_dispatch.rs`** — `root_token` = `HMAC-SHA256(machine key, root.display())`, hex, truncated to
128 bits (`ROOT_TOKEN_BYTES`); `machine_key` reads-or-creates 32 `getrandom` bytes at
`$GRIM_HOME/hooks/root-key`. The create path is **`create_new` (`O_EXCL`), not `atomic_write`**, for
two independent reasons: `atomic_write` caps at `0o644` and would publish the key (T5), and `O_EXCL`
makes a concurrent first install *adopt* the winner's key rather than replace it — replacing it
orphans every token already written into a registration. A short/corrupt key is `InvalidData`, never
a silent re-key. `ensure_hooks_dir` creates `hooks/` `0o700` (W3) and never *loosens* an existing
mode, since C-017 cause 5 is still deferred. `read_table` implements W2 in order — size cap **before**
the read, untyped parse, `schema` (so a *newer* schema reports `UnknownSchema` rather than
`Unparsable`), then whole-table row re-checks (`MATCHER_MAX_BYTES`, absolute `payload_dir`); never
`Err`, no `unwrap`. `converge_root` takes `AdvisoryFileLock` around the read-modify-write, maps
`Locked` → `DispatchError::Locked` without writing, deliberately **replaces** a corrupt table rather
than leaving the corruption armed, and narrows the file to `0o600` after `atomic_write`.

**`hook_launcher.rs`** — `registered_command` emits the corrected five-line form verbatim per
`Vendor::hook_registration`; `registered_command_powershell` mirrors it clause for clause and uses
**`-LiteralPath`** (plain `-Path` treats `[`, `]`, `*`, `?` as wildcards — the Windows analogue of the
word-splitting hole B2 closes). `verdict_arms` renders the empty allowlist into both dialects from the
one table. `shim_body` keeps `exec` (correct there) and no `$PATH` fallback (W9). `generate` is
idempotent on **bytes *and* mode** — bytes alone would report a shim whose `chmod` failed as current,
which is the one failure that makes a hook silently never fire (S1) — narrows `hooks/` and
`hooks/bin/` to `0o700`, and `chmod`s `0o755` as a separate, never-ignored step.

**`hook_registrar.rs`** — `validate_grim_home`: absolute check, then containment on the **resolved**
path (`dunce::canonicalize`, falling back to lexical for a not-yet-created root, since refusing there
would be fail-closed against an ordinary situation). **Cause 2 is checked at both scopes and `scope`
is deliberately unused**: the table is machine-global, so a `$GRIM_HOME` nested in *any* repository
makes the arming authority repo-resident, and narrowing to project scope would let
`grim install --global` run from inside that repository arm the exact shape cause 2 refuses.
`arming_refusal` re-derives causes 1–3 read-only from `$GRIM_HOME` alone (WP-H's seam);
`path_is_representable` was split out of `CommandSpec` so it can, since a status caller has no client,
event or token. `desired_entries` projects state → dispatch set, skipping a native-only moment with a
`debug!` rather than substituting a canonical event, and sorting for byte-stable re-writes. The
git-exclude pair handles **both `.git` shapes** — a linked worktree's `.git` is a *file*, so the real
target is `$GIT_DIR/info/exclude`; a plain `workspace.join(GIT_EXCLUDE_RELATIVE)` would silently miss
it (and that constant now says so). `AlreadyTracked` costs one `git ls-files` subprocess, gated on the
file existing at all, with any git failure answering "not tracked" so the rule is still appended.
`log_sync` was added so a `NotArmed` refusal is a **`warn!`** naming the client and the cause, not a
`debug!` line invisible at the default filter — C-017's whole point (the three vendors now call it).

## 5. `expect(dead_code)` discipline

**29 attributes deleted** (their items went live), and **9 rewritten as
`#[cfg_attr(not(test), expect(dead_code, …))]`** — the items this module's own tests exercise, where an
unconditional `expect` is fulfilled in the production profile and *unfulfilled* under
`--all-targets`. No repo precedent for that form; it is the only shape that keeps the REMOVAL TRIGGER
text visible without warning in one profile or the other. Every placement was settled by the compiler,
as predicted.

## 6. Tests added — 40

`hook_dispatch` 13: token stability / distinctness / hex shape / never the two forbidden wire forms;
per-machine-key distinctness; `0o700`+`0o600` modes; truncated key reported not regenerated; the five
degrade shapes; oversize refused before the read; one bad row rejects the whole table (both
re-checks); write → `Unchanged` → `Removed` → `Unchanged`; other roots preserved verbatim; held lock
reported with **nothing written**; a `0o644` table narrowed on write; a corrupt table replaced.

`hook_launcher` 11: POSIX quoting over all eight shapes WP-P0 executed; PowerShell quoting; the exact
five-line string with the five findings asserted individually; the **C-018b pinning shape** — a
`$(touch pwned)`-laden `$GRIM_HOME` changes only the bytes inside the single quotes, every other line
byte-identical; control character refused in either path and in both dialects; the PowerShell clause
order; all three v1 verdict allowlists empty (+ unknown client fails safe); the shim's exact body with
no `$PATH`; `generate` idempotent and `0o755`; a lost exec bit rewritten; a control-char grim path
refused with nothing written.

`hook_registrar` 16: cause 1; cause 2 at both scopes; a **symlinked** `$GRIM_HOME` into the workspace;
the arming case; cause 3 via `arming_refusal`; all four causes have distinct, library-style messages;
the no-op guard's three branches for splice and its three for own-file; `root_scope_for`; exclude
add → idempotent → remove → absent with the user's own rules surviving; a missing exclude file
created; **a linked worktree writing to its own git dir**; a non-git directory arming anyway; a
tracked settings file reported as `AlreadyTracked`.

## 7. Carried forward

- **Unchanged from Stub:** W3's refusal (C-017 cause 5), S1's launcher verification, S2, S3's golden
  fixtures (now cheap — the string is pinned byte-for-byte in a unit test; S3 promotes that to an
  acceptance fixture), WP-B's `$SHELL` watchlist entry.
- **D-I-1, D-I-3, D-I-4** (stale contract text in `vendor.rs` / `oci/hook.rs`), **D-I-5**'s
  `hook_config_path` trait promotion, **D-I-9**'s duplicate `posix_single_quote` deletion, and
  **D-I-12**'s missing `RootToken` constructor — all in `vendor.rs`, all owed to WP-F implement or
  WP-J2.
- **`ExperimentalOptions::hooks_enabled`**'s `expect(dead_code)` (`src/config/declaration.rs`, WP-E's
  file) still stands: its first caller is step 3 of the convergence, which is WP-J2.
- **WP-J2 must**: replace `owns_anything`'s splice probe with `owned_nested_handlers`, call
  `desired_entries`' corrected signature, and turn `sync_for_state`'s refusal into the real steps 2–6
  — the refusal is the *floor*, not the destination.
- **WP-J2 or WP-K owns the `persist_grant` config lock** (D-I-17). Whoever composes
  `Arming::ConsentRequired` → prompt → grant at the command boundary takes
  `ConfigFileLock::try_acquire` around it. WP-G's `persist_grant` doc should stop naming
  `sync_config` as its caller.
