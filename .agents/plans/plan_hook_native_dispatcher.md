# Plan: Native `grim-hook` dispatcher

## Status

- **Plan:** plan_hook_native_dispatcher
- **Active phase:** 6 — integration gate, review, fix loop (done)
- **Step:** /hex-finalize — handoff (PR #246 ready; merge is the owner's)
- **Last update:** 2026-10-26
- State:   landing
- Tier:    high
- Tier-grammar: 5
- Updated: 2026-10-26
- Next:    owner merge of https://github.com/grimoire-rs/grimoire/pull/246

---

## Header

- **ADR (Accepted 2026-10-25):** [`adr_hook_native_dispatcher.md`](../adr/adr_hook_native_dispatcher.md). It covers O1–O7, H1–H4, C-N1…C-N14 and D1–D3, as amended 2026-10-25: hooks are experimental, so O6 (hard backwards compat) and H3 (version stamp) are withdrawn and H4 deletes `grim hook run`. This plan reopens none of them; it decomposes them.
- **Research:** [`research_hook_dispatcher_ocx_shim_precedent.md`](../research/research_hook_dispatcher_ocx_shim_precedent.md). WP-0 adds `research_hook_native_dispatcher_wp0_probes.md`, which records the evidence.
- **Branch:** `feat/hook-native-dispatcher`. It holds the ADR, this plan and every implementation commit. The root orchestrator runs in the main checkout, and only pipeline workers get worktrees under `.agents/worktrees/`.
- **Tracking:** latency [grimoire-rs/grimoire#87](https://github.com/grimoire-rs/grimoire/issues/87); related [#97](https://github.com/grimoire-rs/grimoire/issues/97), [#168](https://github.com/grimoire-rs/grimoire/issues/168), [#171](https://github.com/grimoire-rs/grimoire/issues/171).
- **Classification:** large scope · one-way door (high: frozen argv, layout, exit table, registration shape) · tier high (`architect=inline`, accepted ADR; `research=skip`; `adversary=off` at plan time, with the code-diff adversary at `/hex-review`).
- **Gate command** (every step): `env -u OTEL_EXPORTER_OTLP_ENDPOINT -u OTEL_EXPORTER_OTLP_HEADERS task verify`, or the subsystem gate (`task rust:verify`, `task claude:tests`) mid-step. The developer shell exports a live OTLP endpoint.
- **Discover inputs:** these scratch maps are not committed, and the facts they hold are cited inline below.
  - `discover_hook_runtime_map.md`
  - `discover_build_release.md`
  - `discover_runtime_split.md`
  - `discover_install_registration.md`

## Objective

Replace the `sh` → shim → 26.8 MB `grim hook run` chain with one native, sub-MiB `grim-hook` binary:
- built from a sibling crate in the same cargo job and embedded in `grim`;
- written to `$GRIM_HOME/hooks/grim-hook[.exe]` by stage + rename iff the on-disk bytes differ;
- registered as bare argv (exec form where the client has one).

At the same time:
- delete the old sh shim and the `grim hook run` verb (experimental: no backwards compat);
- stop masking client fail-closed policy;
- make Windows armable per client once verified.

## Scope

**In:** ADR WP-0…WP-5. This covers:
- the workspace split and embed;
- the core move and the std port;
- the launcher binary;
- install-side `ensure`, shim removal and registration;
- the Windows and macOS gates;
- docs, rules, catalog and the bench gate.

**Out** (ADR § Explicitly out of scope):
- fire-time consent ([#171](https://github.com/grimoire-rs/grimoire/issues/171));
- grim-free plugin runtime ([#168](https://github.com/grimoire-rs/grimoire/issues/168));
- Windows process-tree kill, job objects and `SetDefaultDllDirectories`;
- Claude `onFailure: "block"`;
- payload-owned failure policy (D2 follow-up ADR; P3 files the issue).

## Decisions taken while decomposing

These are decomposition decisions inside the ADR's envelope. None reopens an ADR choice.

| # | Decision | Why |
|---|---|---|
| PD-1 | **WP-1a moves the dependency-light runtime only.** That is types, consts and projection from `oci/hook.rs`, the dispatch *reader*, `envelope`, `projector`, `audit` and `argv::canonical_event`. `pipeline.rs` and `run.rs` move in WP-1b, when they are ported. | Core is declared `serde` + `serde_json` only (C-N1). A "pure move" of `pipeline`/`run` would drag `tokio` and `globset` into core (discover_runtime_split finding 2). |
| PD-2 | **Core may carry `tracing` (facade) and `chrono` temporarily**, in WP-1a only, marked `ponytail: temporary until WP-1b`. WP-1b removes both (warnings become values, C-N9; `core::time`, C-N8). The dep-allowlist test lands in P3, after they are gone. | Hat 1 must not rewrite 6 warn sites and the audit timestamp. Hat 2 does it once. |
| PD-3 | `RootToken` gets **one** public core constructor, `RootToken::parse(&str) -> Option<Self>`, which accepts exactly `[0-9a-f]{32}`. Only grim's HMAC mint calls it, after hex-encoding. **The core reader keeps a crate-private unchecked constructor**, so `read_table` still wraps any stored key exactly as `hook_dispatch.rs:804` does today (Hat 1: non-hex roots are not dropped). `for_test` moves behind a core `test-util` feature that only grim's `[dev-dependencies]` enable. | Minting stays in grim (C-N1) but the type moves, so a constructor must cross the crate boundary. B3's intent (no unvalidated token from outside the reader) is preserved. |
| PD-4 | Before any move, a **byte pin of `grim schema --kind hook`** lands as a characterization test. Moved types keep their `///` comments and serde attrs verbatim; no `code-docs` shortening happens in P1. | That schema is a stable surface (`stability.md:177`) with no byte pin today. schemars copies doc comments into `description`. |
| PD-5 | **Withdrawn 2026-10-25.** `grim hook run` is deleted, not kept. It stays only as the P1–P2 test vehicle, unchanged, until P3b. | Experimental verb; no compat. |
| PD-6 | **D2 concretely:** "a selected gatekeeper is withheld" governs all three causes. The launcher exits **74** when the cause is an unwritable audit (C-012) or a failed `payload = "file"` temp write, and **1** when it is a failed envelope build. **Precedence:** if any gatekeeper that did run composed a `deny`, the deny is projected with exit 0, because an error exit would turn a block into a non-blocking error in Claude. The error exit applies only when no deny was composed. If only observers or mutators are withheld, today's behaviour holds (exit 0, rewrite dropped). | D2 covers "grim-owned infrastructure failures that withhold a selected gatekeeper", and nothing wider. Never weaken a composed deny. |
| PD-7 | The `cfg!(unix)` → `HOOKS_WINDOWS_VERIFIED: &[Vendor]` refactor (empty list, per-client check inside the converge loop) lands in **P4**, because it edits the registrar. **P5** only adds clients and the Windows home check. | P4 and P5 would otherwise both edit `hook_registrar.rs`. |
| PD-8 | The second `converge_root` call (`hook_registrar.rs:615`) takes the same `&guard`. `disarm_all` stays lock-free. `ensure` and the old-shim removal run only when `!union.is_empty()`, never on the fast path, a pure reap or a `disarm_all` path. | The guard is non-reentrant (discover_install_registration). That condition matches today's shim behaviour. |
| PD-9 | **WP-0 verdicts bind later pipelines:** <ul><li>Claude/Qoder exec form falsified → P4 ships the C-N13 shell form for them.</li><li>Copilot drops Claude `args` → Claude ships the shell form.</li><li>Copilot's own exec form falsified → **stop** and raise an `## Actions` line (the ADR gives no fallback).</li><li>The Windows single rename fails → the two-rename path is implemented, and the O6 deviation goes to the owner as an `## Actions` line. It does not block Unix.</li><li>The nested build fails on any target → **stop**; H1 reopens.</li><li>`cargo-auditable` forwarding and member-SBOM shipping (C-N2.2/.5): the verdict is consumed by P3 step 4 (C-024).</li></ul> | This is the ADR's WP-0 gate, made executable. |
| PD-10 | **The launcher acceptance suite is split off into P3b** (wave 5, after P3 and P4). It drives a real `grim install`, so it needs P4's `ensure`. P3 keeps the core and launcher unit tests, which exec the built launcher from `target/grim-hook-embed` against hand-written tables. | Removes the hidden P3→P4 edge, and gives each test file one owner per wave. |
| PD-11 | **Windows doc wording is owned by P5**, not P6. That covers `hooks.md:42-45`, the `json-interface.md` `platform-unsupported` row and the `subsystem-file-structure.md` Windows sentence. It is written last in P5, from the clients P5 actually admitted. | Removes the hidden P5→P6 edge. |
| PD-12 | **`read_table` / `read_stored` stay** for the writer, `status.rs:1159` and `install.rs:473`; the launcher does not use them. `table::select` is additive, and `core::dispatch` uses it. | One fire-time reader, with no collision with P4's writer. |

## Component contracts

The C-N column cites the ADR contract that owns the detail; a contract here restates only what a tester needs.

| ID | Surface | Contract (testable) | C-N |
|---|---|---|---|
| C-001 | Workspace | <ul><li>Root `Cargo.toml` has `[workspace] members = [".", "crates/grimoire-hook-core", "crates/grim-hook"]`, `exclude = ["external/*"]` (if WP-0 shows auto-membership) and `resolver = "3"`.</li><li>`[workspace.package]` is the only version site, and `cargo set-version` bumps it (WP-0 verifies).</li><li>`[workspace.lints]` carries today's `[lints]`.</li><li>Each member has `[lints] workspace = true` and `publish = false`.</li><li>`grim-hook` sets `[package.metadata.dist] dist = false`.</li><li>The root `[dependencies]` opentelemetry pins stay literal (`telemetry.rs:457` reads them).</li></ul> | C-N1 |
| C-002 | `build.rs` | <ul><li>Runs `$CARGO build -p grim-hook --profile {hook\|hook-dev} --target $TARGET --locked --target-dir <T>/grim-hook-embed`.</li><li>`<T>` is `CARGO_TARGET_DIR` resolved against the **package root** when relative (xwin uses `target/xwin`), else derived from `OUT_DIR`; never a literal `target/`.</li><li>`--offline` is passed iff `CARGO_NET_OFFLINE` is truthy.</li><li>The environment is stripped per the C-N2 denylist. Rustflags are re-supplied minus `-Cinstrument-coverage` and `-Cprofile-generate*`, plus `--remap-path-prefix` for `$CARGO_HOME` and the workspace root.</li><li>Emits `GRIM_HOOK_BIN`, plus the `rerun-if-changed` / `rerun-if-env-changed` lists.</li><li>A nested failure fails the outer build with the nested stderr.</li></ul> | C-N2 |
| C-003 | Embed | <ul><li>`pub static LAUNCHER_BYTES: &[u8]`.</li><li>In release, `const _: () = assert!(LAUNCHER_BYTES.len() <= LAUNCHER_SIZE_BUDGET)`, where the budget is 512 KiB (640 KiB on `target_env = "musl"`) and lives in core.</li><li>Debug builds embed `hook-dev` bytes and skip the assert.</li></ul> | C-N2, C-N3, D1 |
| C-004 | Profiles | <ul><li>`[profile.hook]`: inherits release, `opt-level="z"`, `lto="fat"`, `codegen-units=1`, `panic="abort"`, `strip="symbols"`, `debug=false`, `incremental=false`.</li><li>`[profile.hook-dev]`: inherits dev, `panic="abort"`.</li><li>`[profile.dist]` is unchanged.</li><li>No post-link mutation of the launcher.</li></ul> | C-N3 |
| C-005 | `core::stamp` | **Withdrawn 2026-10-25** (no version stamp; `ensure` compares bytes). | C-N5 |
| C-006 | `core::argv` | <ul><li>`render(client, event, root) -> [String; 3]` and `parse(&[OsString]) -> Result<Invocation, ArgvError>`.</li><li>Grammar: client `[a-z][a-z0-9-]{0,31}`, event `[A-Za-z][A-Za-z0-9._-]{0,63}`, root `[0-9a-f]{32}`.</li><li>Exactly 3 operands, no flags, no `--`. Anything else, including an empty client, is `ArgvError` (exit 64).</li><li>An event that passes the grammar but maps to no canonical event is **not** an error: it selects nothing and exits 0 (`test_hook_run_runtime.py:466-478`, `PermissionRequest`).</li><li>A round-trip property test covers it, plus numeric-looking tokens (`000…0`, `1e10…`) arriving verbatim through each client form.</li></ul> | C-N4 |
| C-007 | `core::exit` | <ul><li>`DispatchExit::{Success=0, Failure=1, UsageError=64, DataError=65, IoError=74, NotFound=79}`, named after grim's `cli::exit_code::ExitCode` (`src/cli/exit_code.rs`; 79 is `NotFound`, not `ConfigError`).</li><li>A mechanical equality test checks each value against grim's enum.</li><li>No variant is 2.</li></ul> | C-N7 |
| C-008 | `core::table::select` | <ul><li>`select(bytes, &Invocation) -> Result<Selection, TableError>`.</li><li>Parses the top level with `roots` as `RawValue` and accepts `schema` ≤ `DISPATCH_SCHEMA` (1); a higher value is `TableError::Schema` (65).</li><li>Over `MAX_TABLE_BYTES`, unparsable, `schema` missing or not an integer, or `roots` not an object → 65.</li><li>Unknown root → empty selection (exit 0).</li><li>Each raw row is probed for string `client` and `event`. A failed probe or a mismatch is skipped. A selected row must fully parse, or the result is 65.</li><li>The matcher is evaluated last.</li><li>No reader type is `deny_unknown_fields` (test).</li></ul> | C-N10 |
| C-009 | `dispatch.json` writer | <ul><li>`converge_root(&guard, …)` takes the guard (the lock moves out of it).</li><li>No new field; other roots and unknown keys survive verbatim.</li><li>A no-change rewrite is byte-identical.</li><li>`schema` stays 1.</li></ul> | C-N10 |
| C-010 | `core::glob` | <ul><li>`matches_tool(matcher, tool)`: split on `\|`.</li><li>An alternative with no `*` or `?` is an exact compare. Otherwise it is an iterative glob where `*` is any run and `?` is one char: case-sensitive, no escapes, no separator semantics.</li><li>An empty alternative matches nothing.</li><li>Every case of today's `the_matcher_dialect_is_an_exact_name_or_a_glob_never_a_regex` is ported, and added cases agree with globset in a grim-side differential test over the allowed charset.</li></ul> | C-N8 |
| C-011 | `core::time` | <ul><li>`rfc3339_utc(SystemTime) -> String` formats `%Y-%m-%dT%H:%M:%SZ` (second precision).</li><li>A grim test asserts equality with chrono over a sweep: the epoch, leap days, the year 2100, and ±10⁹ s.</li></ul> | C-N8 |
| C-012 | `core::pipeline` (std) | Behaviour-equivalent to today on each pinned semantic. Each semantic and its test: <ul><li>Spawn forms: payload-dir expansion including argv[0]; `/bin/sh -c` or `cmd /C`, not pre-expanded. Pinned by the payload-dir and shell-form tests.</li><li>Environment: the allowlist in order, parent inherited, a non-flat value dropped. Pinned by c002_i6 and the payload-file variable tests.</li><li>`current_dir` set to `payload_dir`.</li><li>stdin: write-all on a writer thread, broken pipe ignored.</li><li>stdout: `take(64 KiB)`, excess dropped, child not killed. **New test.**</li><li>stderr goes to `null`. **New test.**</li><li>Timeout: `min(entry, deadline-now)`; zero skips the spawn; kill+wait; never joins the reader or writer after a kill. **New test:** `sleep 300 & echo x` returns within the deadline.</li><li>Invocation budget, with `DEADLINE_MARGIN` 500 ms. Pinned by the 4 deadline tests.</li><li>`payload=file` 0o600 `create_new`, removed after reap.</li><li>Ordering O1/O3/O4 and C-011.</li><li>C-012 audit failure policy.</li><li>Audit record fields, rotation and elision.</li></ul> | C-N8 |
| C-013 | Diagnostics | <ul><li>Core returns `Vec<Diagnostic{level, message}>`.</li><li>grim renders it via `tracing`. `grim-hook` writes one `grim-hook: warning: <msg>` line per warning to stderr, and adds debug lines iff `GRIM_LOG` contains `debug` or `trace`.</li><li>No payload, envelope or env value appears in any diagnostic.</li><li>Source scan: the only `std::env::var` in `crates/` is `GRIM_LOG`.</li></ul> | C-N9 |
| C-014 | `grim-hook` binary | <ul><li>`table = canonicalize(current_exe().parent())/dispatch.json`; the directory is canonicalized, never the leaf.</li><li>Reads only the table and stdin; stdin is read only if rows match, capped at 8 MiB (over the cap → 65).</li><li>Writes only the audit trail and the payload file beside the table.</li><li>Exit table C-N7, plus PD-6.</li><li>On error, nothing on stdout, and exactly one `grim-hook: error: …` line after any warning lines. The usage text is pinned as `grim-hook: usage: grim-hook <client> <event> <root>`.</li><li>A panic aborts.</li><li>Dependency allowlist: `cargo metadata` normal deps ⊆ {core, serde, serde_derive, serde_json, itoa, ryu, memchr, proc-macro2, quote, syn, unicode-ident}.</li></ul> | C-N6, C-N7 |
| C-015 | `launcher::ensure(home, &guard)` | <ul><li>Implements the C-N5 algorithm: recover, `O_NOFOLLOW`+`fstat` classification, write iff the on-disk bytes differ, the Unix stage+rename+fsync, the Windows rename-first plus aside path, and the reap of `.tmp` and `.old` files.</li><li>Classification is one read-only `launcher::inspect(home) -> Inspection`, shared by `ensure` and `grim hook list`.</li><li>Returns `Ensured{state}` or `ArmRefusal::{LauncherUnwritable, LauncherPathOccupied}`.</li></ul> | C-N5 |
| C-016 | Registrar | <ul><li>Order: preconditions → **one** guard → `ensure` → old-shim removal → `converge_root(&guard)` → registrations → second `converge_root(&guard)` (PD-8).</li><li>If the lock is held: `dispatch-lock-held`, and no launcher write.</li><li>The per-client Windows gate sits inside the loop (PD-7).</li></ul> | C-N5, C-N11 |
| C-017 | Registration builder | <ul><li>`Vendor::hook_registration(entry, event, launcher, root, host_os)` drops `table` and is built over `argv::render`.</li><li>Exec form gives `HookCommand::Argv` for Claude, Qoder and Copilot, subject to PD-9. Shell form gives `HookCommand::Shell` for Codex, with the C-N13 shell rules: bare charset per shell; else `posix_single_quote`, or refuse with `launcher-path-unquotable` (Codex on Windows), or PowerShell `& '…'` doubling.</li><li>Serializers: Claude/Qoder `command`+`args`; Copilot `exec`+`args` with no `command`, `bash` or `powershell`; Codex `command`.</li><li>`converge_splice`'s silent `continue` on an unknown command form becomes a `debug_assert!` plus a test.</li><li>Registered paths are never `\\?\` verbatim (dunce-style strip), with a Windows golden.</li><li>`#[expect(dead_code)]` comes off `HookCommand::Argv`.</li></ul> | C-N13 |
| C-018 | Old shim removal | <ul><li>Convergence calls `remove_file($GRIM_HOME/hooks/bin/grim-hook)` under the guard, NotFound ignored.</li><li>`shim_body`, `generate` and the shim tests are deleted; `bin` stays reserved.</li></ul> | C-N12 |
| C-019 | Reserved names | <ul><li>`RESERVED_ARTIFACT_NAMES` gains `grim-hook` and `grim-hook.exe` (len 8).</li><li>A binding with either name is refused at both scopes with the cause `binding-name-reserved`.</li><li>`ensure`, finding a directory at the launcher path, gives `launcher-path-occupied` and writes nothing. No reap, no record rewrite.</li></ul> | C-N5 |
| C-020 | Status causes | <ul><li>`HookArmingCause` gains `LauncherUnwritable`, `LauncherPathOccupied`, `LauncherPathUnquotable`, `GrimHomeShared` and `BindingNameReserved`.</li><li>Each has its kebab token, a distinct message, a remedy keyword (or an exempt-list entry), is non-transient, and appears in `json-interface.md`'s cause table (`test_docs.py`).</li><li>`ALL` grows to 18.</li><li>`PlatformUnsupported`'s message is reworded per client.</li></ul> | C-N14 |
| C-021 | `grim hook list` | <ul><li>Additive top-level `launcher: {path, sha256, state}`.</li><li>`sha256` is always present, `null` when the file is missing or a directory.</li><li>`state` comes from `launcher::inspect` (C-015): <ul><li>`missing` = not found.</li><li>`occupied` = a directory.</li><li>`stale` = any other file (link, non-regular, or bytes that differ from the embedded launcher).</li><li>`current` = same bytes.</li></ul></li><li>It is present on the empty-items path too.</li><li>Read-only: no `ensure`, no write (test: mtime and inode unchanged).</li></ul> | C-N14 |
| C-022 | Codex re-trust warning | At default verbosity, `grim install` prints one warning naming each Codex gatekeeper row whose registration string changed this converge. It prints nothing on a no-change converge. | C-N12 |
| C-023 | Windows gate | <ul><li>`HOOKS_WINDOWS_VERIFIED: &[Vendor]`, initially empty.</li><li>A client is added only with its green Windows acceptance.</li><li>`grim-home-shared`: refused unless the canonical home is local and under `%USERPROFILE%` or `%LOCALAPPDATA%`.</li><li>`grim-hook.exe` links `/DEPENDENTLOADFLAG:0x800`, emitted from `crates/grim-hook/build.rs` (`cargo:rustc-link-arg-bins`, msvc only). The root `build.rs` never reaches the nested link.</li><li>Test: the only `.exe` in the `hooks\` root is the launcher.</li></ul> | C-N14 |
| C-024 | Size + supply-chain CI | <ul><li>A PR job builds `-p grim-hook --profile hook` for linux-gnu, linux-musl and windows-msvc (xwin), fails over budget, and prints the bytes.</li><li>`cargo audit bin` runs on the artifact. That artifact is built with `cargo auditable build` (the embed path strips `RUSTC_WRAPPER`, so an embedded launcher carries no audit data), or the limit is recorded per the PD-9 verdict.</li><li>macOS size and `codesign -v` run in `verify-deep`.</li></ul> | NFR Size, C-N2.5 |
| C-025 | `core::dispatch` | <ul><li>`dispatch(table_bytes, &Invocation, stdin) -> DispatchOutcome{stdout: Vec<u8>, withheld: Option<WithheldCause>, diagnostics: Vec<Diagnostic>}`, where `WithheldCause` is one of `AuditUnwritable`, `PayloadFileWrite`, `EnvelopeBuild` or `ThreadSpawn` (added in the review fix pass; exit 1), set only per PD-6 (a gatekeeper was withheld and no deny was composed).</li><li>Defined in P2. `grim-hook` maps it per C-007 + PD-6. The P1–P2 test vehicle `grim hook run` keeps its current exits until P3b deletes it.</li></ul> | C-N7, D2 |
| C-026 | Frozen-interface pin | <ul><li>One core test, `frozen_interface`, owned by P3, pins the argv grammar (the C-006 regexes, 3 operands, no flags) and every exit number (the C-007 equality test).</li><li>It states the contract: grammar and exits are frozen at graduation and treated as frozen now, only new client/event literals are added; future capability arrives as additive optional `dispatch.json` fields, never argv; the launcher skips rows and roots it cannot read unless the invocation selects them, and a selected unreadable row exits 65 (C-008), which is what keeps an older launcher safe against a newer table; at graduation these move into `stability.md`'s frozen list.</li></ul> | C-N4, C-N7, C-N10 |

## User-experience scenarios

| ID | Action | Expected | Error cases |
|---|---|---|---|
| S-001 | `grim install` in a hook-armed workspace, Unix, fresh `$GRIM_HOME` | `hooks/grim-hook` is written at 0o755 with the embedded bytes. Claude/Qoder/Copilot entries are exec form, the Codex entry is shell form, and `bin/grim-hook` is absent. | Lock held → `not-armed/dispatch-lock-held`, no launcher write. Path is a foreign directory → `launcher-path-occupied`. Unwritable → `launcher-unwritable`. |
| S-002 | A second `grim install` | Registrations, table and launcher are byte-identical (no launcher write); `status` reports not-modified. | — |
| S-003 | Upgrade from a pre-ADR estate (workspace A revisited, B not) | A's registrations are rewritten to the new shape, and `bin/grim-hook` is deleted. B's old 5-line string hits its own missing-shim guard and exits 0: its hooks do not fire until `grim install` runs there. | — |
| S-004 | **Withdrawn 2026-10-25** (monotonic versioning). Two grims may overwrite each other's launcher: accepted, harmless. | — | — |
| S-005 | The client fires an armed event | `grim-hook <c> <e> <tok>` → the projected verdict on stdout, exit 0. | Unknown root, no rows, or no match → exit 0, empty. Bad argv → 64. Table absent → 79. Unparsable or schema too high or a selected bad row → 65. I/O → 74. PD-6 cases. |
| S-006 | The launcher is deleted while still registered | Each client applies its own policy: Copilot `preToolUse` denies, Claude and Codex show an error. `grim hook list` shows `launcher.state = missing`. `grim install` repairs it. | — |
| S-007 | `grim hook list --format json` | Items plus the `launcher` object, with nothing written. | No hooks declared → `items: []` and `launcher` still present. |
| S-008 | A hook is bound as `grim-hook` | `not-armed/binding-name-reserved`; nothing is reaped. | A directory at the launcher path → `launcher-path-occupied`. |
| S-009 | Codex user upgrades | `grim install` warns once, naming the untrusted gatekeeper rows. `status` reports `client-trust-pending` until `/hooks` approval. | — |
| S-010 | Windows, client in `HOOKS_WINDOWS_VERIFIED` | Arms via `grim-hook.exe`. Upgrade while running uses rename-aside (or a single rename), and a crash between the renames recovers. | Not in the list → `platform-unsupported`. Shared or UNC home → `grim-home-shared`. Codex with an unquotable path → `launcher-path-unquotable`. |
| S-011 | **Withdrawn 2026-10-25** (`grim hook run` is deleted). | — | — |

## Design guarantees

**Compatibility (Principle 9; hooks are experimental, `stability.md:169-179`).**
- The launcher interface is a future-frozen contract: the argv (C-006) and the exit table (C-007) are frozen from first ship and pinned by C-026. Only new client/event literals are ever added.
- `dispatch.json` keeps its additive-only read rule (C-008): `schema` stays 1, readers ignore unknown keys (a tripwire test asserts no reader type is `deny_unknown_fields`), and unreadable rows and roots are skipped unless selected: a selected unreadable row exits 65 (C-026 as shipped).
- The status and `hook list` `state` and `cause` tokens stay additive-only. Install row `armed` and export hook-admission reasons are untouched.
- No backwards-compat machinery for the old sh launcher or old table readers (owner, 2026-10-25): no kept shim, no stamp, no migration fixture.
- Self-heal is proven by S-002.
- Deliberate behaviour changes. Each gets a release-note line, which P6 writes:
  1. Grim-owned errors now surface to the client's policy (O4, I3 rewrite).
  2. A Codex re-trust is needed once (C-022).
  3. Project-scope bindings named `grim-hook` or `grim-hook.exe` are now refused (`binding-name-reserved`). They were legal before.
  4. Experimental hooks: re-run `grim install` in each workspace; until then its hooks do not fire. The old shim and `grim hook run` are gone.
  5. On the launcher, `GRIM_LOG` loses its EnvFilter target syntax and only `debug`/`trace` substrings count (C-013).
  6. The `dispatch.json.lock` guard is now held across `ensure` and `sync_for_state` (PD-8), so a concurrent install reports `dispatch-lock-held` for longer.
- Rewriting registrations to the new shape is a migration, not a break, because `sync_config` already owns those entries.

**JSON interface.** Additive only, with a null policy of "always present, `null` when unknown" (`skip_serializing_if` is banned in `src/api/`). The tripwires are:
- `HookArmingCause::ALL` length;
- `every_cause_carries_a_distinct_message`;
- `every_actionable_cause_names_a_remedy`;
- `cause_tokens_are_distinct_and_kebab_case`;
- `test_docs.py`'s cause-table check;
- a `hook list` key-set test, which is new because no key-set pin exists today.

**Exit codes.** These are the `grim-hook` exits from C-007, plus PD-6. grim's own commands are unchanged.

**Consumer dispatch rules.** Each client consumes the exits per its own policy, ADR C-N7 per-client table. grim never emits 2 and never forges a 0 for a failure.

**TDD discipline.**
- Every step runs Stub → Specify (red, recorded) → Implement.
- Hat-1 steps (P1) change no test assertion. These are the only allowed test edits:
  - import paths;
  - `include_str!` re-pointing;
  - the new schema pin (PD-4);
  - `for_test` behind `test-util` (PD-3);
  - the `CARGO_MANIFEST_DIR` source-walk extension to `crates/`;
  - dropping `permitted_fields`' `cfg_attr(not(test), expect(dead_code))`, because a `pub` item in a lib crate is never dead, so the `expect` would go unfulfilled.
- Hat-2 steps (P2) record a before/after bench.
- A step whose tests went green without a red run is reopened.

## Contract wave (CW)

CW (wave 3) lands once, after P2 and before wave 4. It adds stubs and red contract tests for everything P3 and P4 share.

- **Core.** Each of these is a signature with an `unimplemented!()` body or a const, plus its red tests:
  - `argv::{render, parse, Invocation, ArgvError}` (C-006). `render` is implemented in CW, because it is trivial and P4 needs it green.
  - `exit::DispatchExit` (C-007), implemented, with the equality test.
  - `table::{select, Selection, TableError}`, (C-008).
  - `LAUNCHER_SIZE_BUDGET`, which already exists from P0.
- **Reader fate (PD-12):** `read_table` / `read_stored` stay; `select` is additive.
- **grim.**
  - `launcher::{inspect, ensure, Inspection, Ensured, LauncherState}` (C-015) stubs.
  - `DispatchGuard::try_acquire(home) -> Result<DispatchGuard, Locked>` (lock moved out of `converge_root`). This is the one adaptation point for the `converge_root` callers.
  - `HookArmingCause`'s five new variants with tokens and messages (C-020), implemented, together with their doc rows. They are small, and P4 and P3 tests need them.
  - The `HookListReport.launcher` field type (C-021) stub.
  - The `converge_root` signature takes `&DispatchGuard` (C-009/C-016). It is a signature change only: callers pass the guard, and behaviour is unchanged.
- **Gate:** `task rust:verify` compiles, every pre-existing test is green, and the new contract tests fail with `unimplemented`.

## Pipelines

| id | repo | scope | expected files | wave | depends-on | marks | status |
|---|---|---|---|---|---|---|---|
| P0 | grimoire | WP-0 spike + workspace skeleton: C-001–C-004; PD-9 verdicts | `Cargo.toml`, `Cargo.lock`, `build.rs`, `crates/**`, `src/install/hook_launcher.rs` (`LAUNCHER_BYTES`), `taskfiles/{rust,coverage}.taskfile.yml`, `taskfile.yml`, `.github/workflows/verify-deep.yml` (nextest `--workspace`), `.licenserc.toml`, `.claude/rules/*.md` paths, `.claude/rules.md`, AGENTS.md, `product-context.md`, `.agents/research/research_hook_native_dispatcher_wp0_probes.md` | 0 | — | hard | merged |
| P1 | grimoire | WP-1a core move (Hat 1): PD-1–PD-4 | `crates/grimoire-hook-core/src/**`, `src/oci/hook.rs`, `src/install/hook_dispatch.rs`, `src/hook/audit.rs`, `src/command/hook/{envelope,projector,argv}.rs`, `src/command/hook.rs` (scans), `src/command/schema.rs` tests | 1 | P0 | — | merged |
| P2 | grimoire | WP-1b std port (Hat 2): C-010–C-013, C-025, with `grim hook run` kept as a thin test vehicle over core until P3b | `crates/grimoire-hook-core/src/{pipeline,run,glob,time,diag}.rs`, `src/command/hook/{run,pipeline}.rs` (thin), `src/main.rs`/`src/app.rs` (F1 removal), bench rows | 2 | P1 | — | merged |
| CW | grimoire | Contract wave (above) | core `argv/exit/table`, `src/install/launcher.rs`, `src/api/{artifact_status,hook_report}.rs`, `src/install/hook_dispatch.rs`, `docs/.../json-interface.md` cause rows | 3 | P2 | — | merged |
| P3 | grimoire | WP-2 launcher binary: C-006–C-008, C-026, C-013, C-014, C-024, C-025 (PD-6 mapping); PD-12 switch; D1 closed; D2 follow-up issue | `crates/grim-hook/**`, `crates/grimoire-hook-core/src/**`, `test/tests/test_hooks_fixes_table_forward_compat.py` (C-008 expectations, through the vehicle until P3b), `.github/workflows/verify-basic.yml` (size job), `verify-deep.yml` (macOS size) | 4 | CW | — | merged |
| P4 | grimoire | WP-3 install side: C-009, C-015–C-022, PD-7, PD-8; S-001–S-003, S-006, S-008, S-009 | `src/install/{launcher,hook_registrar,hook_dispatch,hook_launcher,vendor,vendor_claude,vendor_qoder,vendor_codex,vendor_copilot,client_target,installer}.rs`, `src/oci/hook.rs` (reserved names), `src/command/{status,install,hook/list}.rs`, `src/api/**`. Tests owned: `test_hook_arming.py`, `test_hooks_boundary.py`, `test_hooks_flag_off.py`, `test_hooks_lifecycle.py`, `test_hook_list.py`, `test_hooks_qoder.py` (splice shape), `test_hook_decline_dispatch.py` (:433-507), `test_hooks_port_contracts.py` (:280), plus new `test_hooks_upgrade.py` (shim removal, ensure byte-compare) | 4 | CW | review | merged |
| P3b | grimoire | Launcher acceptance (PD-10): S-005, S-006, C-014 end to end; direct-call migration; delete `grim hook run` | `test/tests/test_hook_launcher.py` (new), `test/src/` launcher helper, `src/command/hook/run.rs` and its clap variant (deleted), call sites: `test_hook_run_runtime.py:79-95`, `test_hooks_copilot.py:74`, `test_hook_decline_dispatch.py:197-209`, `test_hooks_port_contracts.py:411`, `test_hooks_qoder.py:178`, `test_hooks_fixes_remove_disarm.py:78`, `test_example_hooks.py:143-155`; `test_telemetry_contract.py:154` is removed with the verb (the launcher has no telemetry code) | 5 | P3, P4 | — | merged |
| P5 | grimoire | WP-4 Windows + macOS: C-023, S-010; PD-9 Windows rename; PD-11 Windows doc wording | `src/install/hook_registrar.rs` (`HOOKS_WINDOWS_VERIFIED` entries, home check), `crates/grim-hook/build.rs` (link arg), `test/tests/test_hooks_fixes_windows.py`, new `test_hooks_windows_*.py`, `verify-deep.yml` (Windows acceptance + macOS), `hooks.md:42-45`, `json-interface.md` `platform-unsupported` row, `subsystem-file-structure.md` Windows sentence | 5 | P3, P4 | hard | merged |
| P6 | grimoire | WP-5 docs, rules, catalog, bench: the ADR surface list; NFR latency gate | `docs/src/content/docs/{hooks,hooks-troubleshooting,commands,json-interface,configuration,telemetry,stability}.md`, `catalog/**` (skills, hooks README, CHANGELOG), `.claude/rules/{subsystem-file-structure,subsystem-cli-commands,subsystem-ci,subsystem-taskfiles,arch-threat-model,vendor-capability-watchlist,product-tech-strategy,workflow-release}.md` (not the PD-11 lines), `taskfiles/bench.taskfile.yml`, `.agents/hook_dispatch_latency.md`, `CHANGELOG`/release-note draft | 5 | P3, P4 | — | merged |
| R1 | grimoire | /hex-review xhigh convergence gaps (2026-10-26): C-002 contradicts (nested rustflags/remaps dropped); C-023 contradicts (`cmd.exe` binding shadows launcher's `cmd` search); C-026 + Design guarantee "unreadable rows skipped" contradicts (`select` exits 65); ADR Latency NFR contradicts (gate measures Copilot exec form only); D2/PD-6 contradicts (thread-spawn failure and non-object stdin withhold a gatekeeper at exit 0); partial: C-001, C-013, C-014, C-015, C-020, C-021, C-024, C-025, D1, D2 (issue unfiled), PD-9 (macOS/native dist unverified), PD-11, S-001, S-003, S-009, S-010, P5 steps 4–5, P6.2/P6.4; P3b row status stale (landed c84f4b30, 4beac677, 1b9e36da) | `build.rs`, `build/embed.rs`, `crates/grimoire-hook-core/src/{run,pipeline,table,response}.rs`, `src/oci/hook.rs`, `src/install/{launcher,hook_registrar,hook_dispatch}.rs`, `src/api/artifact_status.rs`, `docs/src/content/docs/{stability,hooks,hooks-troubleshooting,json-interface}.md`, `taskfiles/bench.taskfile.yml`, ADR changelog | 6 | P3b, P5, P6 | review | merged |

**Why fewer parallel pipelines than file-disjointness allows:**
- P0 → P1 → P2 are serial by design: a spike gate, then the Two Hats rule (structure before behaviour).
- P3 and P4 are disjoint once CW fixes their shared core surface.

```mermaid
graph TD
  subgraph W0
    P0[P0 WP-0 spike + skeleton]
  end
  subgraph W1
    P1[P1 core move, Hat 1]
  end
  subgraph W2
    P2[P2 std port, Hat 2]
  end
  subgraph W3
    CW[CW contract wave]
  end
  subgraph W4
    P3[P3 launcher binary]
    P4[P4 install side]
  end
  subgraph W5
    P3b[P3b launcher acceptance + delete verb]
    P5[P5 Windows + macOS]
    P6[P6 docs, rules, catalog, bench]
  end
  P0 --> P1 --> P2 --> CW
  CW --> P3
  CW --> P4
  P3 --> P5
  P4 --> P5
  P3 --> P6
  P4 --> P6
  P3 --> P3b
  P4 --> P3b
  P3b --> SA[security-auditor gate]
  P5 --> SA
  P6 --> SA
```

**Critical path:** P0 → P1 → P2 → CW → P4 → P5 → security-auditor gate (unchanged by the slim-down; P3b still follows P3 and P4).
**Shippable after wave: 5.** Wave 4 is functionally complete on Unix, with Windows still `platform-unsupported`. Wave 5 adds the docs and the latency gate a release needs.

### P0 — WP-0 spike and workspace skeleton (`hard`)

1. **Skeleton (C-001, C-004).**
   - Write the workspace manifest and the members.
   - `crates/grimoire-hook-core` holds only `LAUNCHER_SIZE_BUDGET`.
   - `crates/grim-hook` is a stub `main` that reads stdin through `serde_json::from_reader::<serde_json::Value>`, and spawns one `Command`. That gives real deps.
   - Commit `Cargo.lock`.
   - Add `--workspace` to clippy, nextest (taskfile and `verify-deep.yml:37`) and llvm-cov, `--all` to fmt, `crates/**` and `build.rs` to `rust-sources`, and `.licenserc.toml` includes.
   - Gate: `cargo build`, `cargo clippy --workspace`, `task rust:verify`.
2. **Embed (C-002, C-003).**
   - Write `build.rs` per C-002, and `LAUNCHER_BYTES` plus the release const-assert.
   - Unit tests: `LAUNCHER_BYTES` is non-empty; target-dir derivation for set, unset and relative `CARGO_TARGET_DIR`; the env denylist strips each listed variable (factor `build.rs` logic into a `#[path]`-included module so it is testable).
   - Gate: debug and release builds, `task verify`.
3. **Rules and agent config (A11 subset).**
   - Add `crates/**` to the `paths:` of `arch-threat-model.md` and the four `src/**` subsystem rules.
   - Update the `rules.md` overlap rows :142-143 text and the auto-load table.
   - In AGENTS.md :74, `product-context.md` :106-107 and `arch-principles.md` :18, "single binary crate" becomes a workspace with one binary crate plus two hook crates.
   - Gate: `task claude:tests`. The `crates/` directory exists by now, so the dead-glob test passes.
4. **Probes.** Record each result in `research_hook_native_dispatcher_wp0_probes.md`:
   - `dist build` on the host, and on all 8 targets through a branch `verify-deep`/dist dry run. Pushing the feature branch with lease and dispatching are standing grants (memory: do not stall on skill guards).
   - Precise-builds not double-building; member SBOM output.
   - `run.sh` xwin build, plus a zigbuild musl build.
   - `cargo llvm-cov --workspace`; an offline nested build.
   - The Windows running-image rename probe (scratch probe exe via `run.sh`'s `cmd.exe` path).
   - macOS arm64 `codesign -v` plus exec-after-replace (verify-deep).
   - A Claude exec-form probe (real `claude` CLI, `args` hook logging argv, version floor).
   - A Copilot exec probe, and a Copilot run of a Claude exec-form row from global `settings.json` and project `settings.local.json`.
   - The std size floor per target.
   - Gate: the artifact exists with a verdict per PD-9 row. A missing tool (for example the Copilot CLI) **stops** with an `## Actions` line, and nothing is guessed.

### P1 — Core move, Hat 1

1. **Characterization (PD-4).** Golden-pin `grim schema --kind hook` bytes (sha256 plus a fixture file). Gate: green on unchanged code.
2. **Move types and projection.**
   - `CanonicalEvent`, `HookTier`, `HookPayloadMode`, `HookHandler`, the consts, the matcher charset, `VerdictTokens`, `EVENT_ECHO_FIELD`, `ProjectionRow`, `RESPONSE_PROJECTION` and `projection_for` go into core.
   - Schemars derives move behind the `schema` feature, which grim enables.
   - `HookTier::ALL` becomes public and unconditional.
   - `pub use` in `oci/hook.rs`. Tests that need `ClientTarget` stay in grim.
   - Gate: the schema pin is green, and the hook tests are unchanged.
3. **Move the dispatch reader and `RootToken` (PD-3).**
   - `DISPATCH_SCHEMA`, `MAX_TABLE_BYTES`, `DispatchEntry`/`Root`/`Table`, `StoredRoot`, `DispatchDegrade`, `read_table` and `read_stored` go into core.
   - Add `RootToken::parse`. `for_test` moves behind `test-util`.
   - The writer, lock and key code stay in grim. All 24 `hook_dispatch.rs` tests stay in grim against the core reader.
4. **Move `envelope`, `projector`, `audit` and `canonical_event`.**
   - `tracing` and `chrono` become temporary core deps (PD-2).
   - `AUDIT_FILE`, `ROTATED_SUFFIX` and `envelope_file_name` are `pub`.
   - The projector loop over `ClientTarget::ALL` stays in grim.
   - The C-007/C-009 `include_str!` scans are re-pointed at the core paths, with `crate::` needles adapted.
   - Extend the `CARGO_MANIFEST_DIR` source walks (`hook_registrar.rs:2325`, `log_switch.rs:475`) to `crates/`.
   - Gate: every hook unit and acceptance test passes unchanged, and `task verify` passes.

### P2 — std port, Hat 2

1. **Bench before.** Run `task bench:hooks` on the current tree (install `hyperfine` if it is missing) and record `match/1` p50.
2. **`core::time` and `core::glob` (C-010, C-011)**, with red tests first: the ported matcher table, the globset differential (grim side), and the chrono sweep (grim side).
3. **Port `pipeline` and `run` into core on std (C-012).**
   - Spawn, writer and reader threads and the deadline loop.
   - The 12 `#[tokio::test]`s become sync, with unchanged assertions.
   - New tests: stdout cap, stderr null, and the grandchild pipe hold.
   - Warnings become values (C-013). Drop `tracing`, `chrono`, `tokio` and `globset` from core.
4. **Test vehicle.**
   - `src/command/hook/run.rs` becomes argv parsing over `core::dispatch`, with its exits unchanged. It lives only until P3b deletes the verb.
   - The F1 current-thread scan is deleted; B1 goes with the verb in P3b.
   - Gate: the full suite, plus a bench after (`match/1` p50 ≤ before + 1 ms) recorded in `hook_dispatch_latency.md`.

### CW — Contract wave

Single commit, as specified above.

### P3 — Launcher binary

1. **Implement `argv::parse` and `table::select`** (C-006, C-008) against the CW red tests.
2. **Switch `core::dispatch` to `select` (PD-12).**
   - Update `test_hooks_fixes_table_forward_compat.py` to the C-N10 rules 1–4 through the test-vehicle flag form until P3b migrates it to the launcher.
3. **`grim-hook` main (C-014, C-013, C-025 → exit mapping, PD-6).**
   - Table resolution, the exit mapping and stderr lines.
   - Rust integration tests exec the built `target/grim-hook-embed/…/grim-hook` beside a hand-written `dispatch.json`:
     - every C-007 row;
     - unknown root → 0;
     - never 2;
     - empty stdout on error;
     - a launcher replaced mid-resolution resolves the right table;
     - a selected bad row → 65 and a skipped non-selected one;
     - `schema: 2` → 65;
     - each PD-6 case, including a composed deny winning over a withheld gatekeeper.
4. **Guards and CI.**
   - Dependency-allowlist test (C-014), env-var source scan (C-013) and the `frozen_interface` pin (C-026).
   - The CI size job plus `cargo audit bin` (C-024). Dispatch it, watch it, and record the real bytes per target in the probes artifact. That closes D1 (cut order per D1 if over).
   - File the D2 follow-up issue for payload-owned outcomes.

### P3b — Launcher acceptance (PD-10)

1. **Fixture helper.** Add a `test/src` helper that runs `grim install` in an armed workspace and returns `$GRIM_HOME/hooks/grim-hook`. Never copy `target/` artifacts.
2. **`test_hook_launcher.py`.** Covers S-005, S-006 and the C-014 behaviours end to end, through real registrations: the exec form, or Codex's shell via `sh -c`.
3. **Migrate the call sites** listed in the table to launcher argv, then delete `grim hook run`: `run.rs`, its clap variant, the relative-`--table` refusal test (`test_hook_run_runtime.py:340-356`) and the F1/B1 scans. No legacy suite is kept (PD-5 withdrawn).

### P4 — Install side (`review`)

1. **`launcher::ensure` (C-015)**, red first, one test per C-N5 branch:
   - NotFound, directory, symlink, non-regular;
   - a file longer than the embedded bytes, differing bytes, equal bytes (no write);
   - mode/owner repair;
   - Unix new inode (the inode changes; an exec'd copy survives);
   - reap of `.tmp` and `.old`;
   - recover after a crash between renames (simulated).
2. **Registrar order and guard (C-016, PD-7, PD-8).**
   - One guard; `converge_root(&guard, …)` (C-009); the second converge on the same guard.
   - `HOOKS_WINDOWS_VERIFIED` (empty) with a per-client check in the loop.
   - `dispatch-lock-held` → no launcher write (extends the test at `:3183`).
   - Two concurrent installs → one valid launcher and no litter.
3. **Registration builder and serializers (C-017)**, under the PD-9 verdict:
   - goldens per client × OS × exec/shell × bare/quoted;
   - the `client_target.rs:621/625/810` probe update;
   - replacing the Argv `continue` trap;
   - the registrar shell-string assertions at `:3243`/`~3298-3315` and the inline 5-line literals updated.
4. **Shim removal and reserved names (C-018, C-019)**, with the shim-removal fixture (S-003) and the reserved-name refusal (S-008). Extend `test_hook_decline_dispatch.py:433-507` with `grim-hook`.
5. **Causes and reports.**
   - Wire the C-020 cause mappings in `cause_from_refusal`.
   - The C-021 `hook list` launcher object, with a key-set and no-write test.
   - The C-022 Codex warning.
   - Update the acceptance expectations: `test_hook_arming.py`, `test_hooks_boundary.py` (new exec-form driver; `:455-484` reversed to `launcher.state = missing`), `test_hooks_flag_off.py`, `test_hooks_lifecycle.py`, `test_hooks_qoder.py`.
   - Gate: self-heal S-002 (byte-identical second install), then `task verify`.

### P5 — Windows + macOS (`hard`)

1. **Home and link hygiene.**
   - `grim-home-shared` check (C-023).
   - The `/DEPENDENTLOADFLAG:0x800` link arg, from `crates/grim-hook/build.rs`.
   - The "only `.exe` in the hooks root" test.
   - Decide `icacls /inheritance:r` on `hooks\` (C-N14), and record the decision in the probes artifact.
2. **Windows acceptance** via `/mnt/c/Users/ecom/grim-wintest/run.sh`, per client:
   - real launch paths: Claude exec plus the Git Bash and PowerShell fallbacks if PD-9 picked shell, Codex `cmd /C`, Copilot exec;
   - homes with a space and with quotes;
   - rename while a slow hook runs, plus the sharing-violation retry;
   - a Claude row under Copilot;
   - the `grim-home-shared` refusal.
   - Compare against `origin/main` before blaming the branch (known failures: [#159](https://github.com/grimoire-rs/grimoire/issues/159)).
3. **Admit clients.**
   - For each client: add it to `HOOKS_WINDOWS_VERIFIED` locally, run its Windows suite, and keep the commit only if that suite is green. One commit per client.
   - No environment or flag override exists for the gate (the hooks-env rule).
   - Qoder stays gated (unobserved).
4. **Clean-Win11 first run** (D3: Defender block-at-first-sight, Smart App Control). If this blocks, it becomes an `## Actions` line for the owner (signing decision); it is never worked around.
5. **CI.**
   - Add the Windows acceptance step to `verify-deep` (WP-4 "run.sh, then CI").
   - macOS `codesign -v`, exec, replace, exec, plus the size check.
   - Dispatch and watch the run.
6. **Windows doc wording (PD-11)**, written from the clients actually admitted.

### P6 — Docs, rules, catalog, bench

1. **Docs.** Update the pages and lines in ADR § "Docs, rules and config surfaces", keeping every existing `{#anchor}`:
   - `commands.md:1130`: the `grim hook run` section is deleted, and `subsystem-cli-commands.md` loses its row (deleted, not reworded);
   - `hooks-troubleshooting.md` gains the new causes, Claude-on-PowerShell blocking, the Codex re-trust and a "re-run `grim install` after upgrading" note;
   - `stability.md` adds the argv, table and registration-shape contracts;
   - `GRIM_LOG` loses its EnvFilter target syntax on the launcher.
   - `telemetry.md:37` and the AGENTS.md `GRIM_TELEMETRY` row stop mentioning `grim hook run`.
   - The Claude exec-form version floor from the P0 probe.
   - Draft release notes from Design guarantees › behaviour changes 1–6.
   - Gate: `task docs:check` (Node 24), `test_docs.py`, `test_check_urls.py`.
2. **Rules.**
   - `subsystem-file-structure.md` § Hooks: layout, reserved list, Windows.
   - `subsystem-cli-commands.md`: the `grim hook run` row is deleted.
   - `arch-threat-model.md`: I3 rewritten, and the scope note's "exits 0" deleted.
   - `vendor-capability-watchlist.md` :329-330: exec form is now used.
   - `product-tech-strategy.md`: the launcher Tokio deviation.
   - `workflow-release.md:53`: the `[workspace.package]` version.
   - `subsystem-ci.md`: the size job and the Windows/macOS steps.
   - `subsystem-taskfiles.md:87-89`: the `--workspace` example.
   - Gate: `task claude:tests`.
3. **Catalog drift.** Update the ADR-listed `grim-usage`, `grim-authoring` and `hooks/README.md` lines, plus a `catalog/CHANGELOG.md` entry. Gate: `task catalog:verify`.
4. **Bench gate.**
   - Retarget `bench.taskfile.yml`: launcher argv via `hyperfine -N`, the table beside the launcher, evict lists naming the launcher, a `hook` profile assert, and a header reflecting that the latency row now gates.
   - Run on WSL2 ext4: cold `no-match/matcher×1 · chain` p50 ≤ 7.47 ms, and warm ≤ 2.65 ms.
   - **Failing this falsifies the size premise:** stop and report, never tune the bench.
   - Record measured rows for every other platform reachable here (the Windows host through `C:`). Mark every other platform unmeasured, never inferred (#87).

### Integration gate — `/security-auditor`

After P3b, P5 and P6 merge, and before `/hex-review`, run `/security-auditor` over the branch diff (ADR § Validation). It covers:
- the new executable written to disk;
- `ensure`'s link and permission handling;
- the reserved-name refusal and `launcher-path-occupied`;
- the I3 rewrite;
- the Windows DLL and home checks.

Findings feed the review-fix loop.

## Testing strategy (coverage join)

| Contract | Unit | Acceptance |
|---|---|---|
| C-001–C-004 | build.rs module tests, LAUNCHER_BYTES non-empty | `task verify` both profiles; P0 probes |
| C-006–C-008, C-013, C-014, C-025, C-026 | core tests, round-trip property, `frozen_interface` pin, launcher exec tests | `test_hook_launcher.py` (S-005, S-006) |
| C-009, C-015, C-016 | ensure branch tests, registrar order | S-001, S-002, S-006 fixtures |
| C-010–C-012 | ported pipeline tests (+3 new), differential, sweep | the existing hook suite, unchanged through P2, then on the launcher (P3b) |
| C-017, C-018, C-022 | goldens, shim-removal test | S-003, S-009 |
| C-019 | reserved-name tests | S-008 |
| C-020, C-021 | cause tripwires, report key-set | `test_hook_list.py`, `test_docs.py` (S-007) |
| C-023 | home check, `.exe` hygiene | Windows suites (S-010) |
| C-024 | — | CI size job, `verify-deep` |

## Risks

| Risk | Mitigation |
|---|---|
| Nested build breaks on an exotic cross setup | P0 probes all targets before any move; PD-9 stop rule |
| The size budget is missed with real dispatcher logic | D1 cut order; real bytes measured at the end of P3 |
| Claude exec form falsified, or Copilot drops `args` | PD-9: the shell form is decided before P4 ships the shape |
| Hat-1 move silently changes the published hook schema | PD-4 byte pin before P1 step 2 |
| The thread-based deadline loop regresses latency | P2 before/after bench, ≤ 1 ms gate |
| Windows running-image rename window | P0 probe; any deviation from O6 goes to the owner; Unix is not blocked |
| Shared-file conflicts between same-wave pipelines | CW fixes `converge_root`, `DispatchGuard` and the core APIs. Test files have one owner per wave (the table). P3b runs after P4. P5 owns the Windows doc lines (PD-11) |

## Rollback

Everything ships in one release behind the existing experimental hooks flag (I4).

**Before release:** revert the branch.

**After release, at runtime:** turning the flag off disarms everything, and unknown or flag-off roots exit 0. The old shim and `grim hook run` are gone, so reverting means running the older grim's `grim install` again in each workspace (experimental surface).

## Open questions

None. D1–D3 were resolved at the gate. The PD-9 verdicts are evidence-driven, not owner questions, except for an O6 deviation, which P0 escalates as an `## Actions` line if the Windows single rename fails.

## Progress Log

| Date | Update |
|------|--------|
| 2026-10-25 | Plan written by `/hex-plan` (high) from the Accepted ADR and three discover maps |
| 2026-10-25 | Slimmed: owner withdrew backwards compat (hooks are experimental). Withdrawn: O6, H3, PD-5, C-005, S-004, S-011. Reworked: PD-8, PD-12, C-008, C-009, C-015, C-016, C-018, C-019, C-021, S-001–S-003, S-008, P2 step 4, P3b (now also deletes the verb), P4 steps 1 and 4, P6 docs. P3b kept (native-launcher suite). Added C-026 (frozen-interface pin) |
| 2026-10-26 | /hex-execute P0 landed (7953d166, 662c2442, 0c7c6ae0, ac188425). WP-0 evidence: `research_hook_native_dispatcher_wp0_probes.md`. **PD-9 decisions** — Q: which registration form per client? Research: Claude runs `command`+`args` as a direct exec (2.1.310), but Copilot 1.0.91 fires Claude rows from project `.claude/settings{,.local}.json` with argc=0; Qoder unprobed (CLI not logged in); Copilot's own `exec`+`args` holds. Decision: **Claude and Qoder ship the C-N13 shell form** (Qoder follows Claude: its exec form is unverified and shell form is proven); **Copilot ships exec form**; Codex shell form. Q: Windows replace of a running exe? Research: single rename fails with error 5; two-rename aside works, `.old` deletable after exit. Decision: implement the C-N5 two-rename path; no owner escalation, since O6 (the atomic-replace intent) was withdrawn 2026-10-25. Q: cargo-auditable (C-N2.2/.5)? Decision: build.rs forwards `RUSTC_WORKSPACE_WRAPPER` only when it is cargo-auditable (launcher gains `.dep-v0`); member SBOMs not shipped (dist publishes only `grimoire.cdx.xml`; mitigations `.dep-v0`, `cargo audit bin`, `hook list` sha256). Open: macOS + native non-host dist runners need a CI run (remote act, reported to the lead); local gnu/musl/msvc/aarch64 cross-builds green |
| 2026-10-26 | P1 landed: f9aa5ff0 (schema pin), af7eff03, 7157273c, 8b385f94; fix 5e04b06c (a test-only `mod` at the top of `main.rs` hid production lines from the hook source scans). Unit suite 4549/4549, hook acceptance 174 passed. Beyond plan: `Decision`/`CanonicalResponse` moved to core `response.rs` (the projector takes them); core carries `tracing` + `chrono` per PD-2 until P2 |
| 2026-10-26 | P2 landed: be5df11b (C-010/C-011), 0eaac745 (std port, C-012/C-013/C-025; core deps now serde + serde_json only), 9bdd50fa (thin `grim hook run`, F1 removed). Unit 4565/4565, hook acceptance 174 passed. Bench gate (`match/1` runtime warm p50 ≤ before + 1 ms): 6.45 → 6.34 and 6.20 ms on two runs, **pass**. The `chain` rows (sh shim → `grim hook run`) read 2–4 ms slower on both runs; Q: regression to chase? Decision: no — the chain is deleted in P3b and the P6 latency gate measures the launcher; P3 watches the reap poll (`exited_within`, 0.2→5 ms backoff) in its launcher numbers. Red-first: P2 step 3 wrote its 3 new tests after the port, and proved them by four planted mutations instead (recorded as the red evidence) |
| 2026-10-26 | CW landed 812a1fb8. P3 landed (merge on feature branch): 4f72cbc5, bb0dac12, 5118f8ea, 0cf31618 (D1 size cut: manual Value extraction, mpsc→Arc<Mutex>+park_timeout), 6f7b30a2, 952fe51d. **D1 closed locally**: launcher bytes gnu (auditable) 506,296 / musl 610,976 / msvc 346,112 vs budgets 524,288 / 655,360 / 524,288; macOS unmeasured until verify-deep runs. Q: dispatch the size job (plan says dispatch+watch)? Decision: no remote grant in this run — reported to the lead; local numbers stand until CI confirms. Q: allowlist set? Research: real normal-dep tree is serde, serde_core, serde_derive, serde_json, itoa, memchr, zmij, proc-macro2, quote, syn, unicode-ident (no ryu); `cargo metadata` unifies features workspace-wide and pulls schemars. Decision: test uses `cargo tree -p grim-hook -e normal --target all`, allowlist = the real set. Wire change: a response given as a JSON array is now rejected (the old derive accepted it by accident). D2 follow-up issue → reported to the lead (no issue grant). P4 landed: 52b48d6d, c5e05251, ba979579, a8abcbad, a0164b97, a8fcf971, 5dbe8c2f, f85e7593. Unit 4658/4658, hook acceptance 189 passed 3 skipped. Q: `grim status` with a dir at `hooks/grim-hook`? Decision: status inspects the launcher read-only and reports `launcher-path-occupied`; kept out of `arming_refusal` so uninstall passes aren't blocked. `grim-home-shared` keeps its dead-code expect until P5. Open: no acceptance test for the positive Codex re-trust warning (needs a forged owned-file record; unit-tested) |
| 2026-10-26 | P5 landed (396399ad … b31a2d6d): `grim-home-shared`, `/DEPENDENTLOADFLAG:0x800`, rename-aside sharing-violation retry (bug found by provoking error 32), status fixes for unadmitted/unquotable clients, `hooks-windows` CI job + macOS codesign/exec/replace steps. icacls: not applied (evidence in the probes artifact). **Q: keep Copilot and Codex in `HOOKS_WINDOWS_VERIFIED`?** Research: neither CLI is installed on the Windows host; both passed only emulated launches (CreateProcess of `exec`+`args`; `cmd.exe /C`). Claude passed a live `claude -p` run. Copilot denies a failing `preToolUse` (fail-closed), so an admission whose real launch path is unobserved can block every tool call (I3). Decision: **admit Claude only**; revert the Copilot and Codex admissions and their doc wording; live verification of both becomes a needs-issue item. Strong reason recorded for diverging from step 3's per-client list. P6 landed (b8f854c4, a9accff3, 58c0b550, 8e6da8fa). Bench gate **PASS**: chain no-match/matcher×1 cold 1.81 ms (≤ 7.47), warm 0.87 ms (≤ 2.65). `task docs:check` site build green; its trailing `npm audit` step fails on a pre-existing zod advisory in `astro-rehype-relative-markdown-links` (not this branch). No `catalog/CHANGELOG.md` exists (catalog ships the root git-cliff CHANGELOG): no entry, release callouts staged in `release_notes_callouts_next.md` instead |
| 2026-10-26 | Integration gate ∥ review. Resume merged the Windows dead-code fix (e7660c9b) and the P3b row; `hook run` mentions outside history are gone (b5064b6e). Gate 1 `task --force verify` green (2156 passed). `/security-auditor`: no Block; one Warn (CWE-427, an executable-suffix binding name shadowing `cmd` in `hooks\\` on Windows), fixed. `/hex-review` xhigh: Request Changes, 4 Blocks (nested rustflags dropped by the env deny-list; non-object stdin and thread-spawn failure forging exit 0 for a gatekeeper; the `cmd.exe` shadow) + ~26 Warns, recorded as row R1. One fix pass (6 parallel fixers + one follow-up): 933bc379, 0b205da8, 676100d1, 0841c3b5, 34d0ce8e, fd9a084e, a6b91ff3, fe1948e6, 46ce07b1, 89190add, 512f25d1. Launcher bytes after the remap fix: gnu 503,712 / musl 610,976. Gate 2 `task --force verify` green (2159 passed), `task catalog:verify` green. Windows local suite: 8 failed + 2 errors under `-n auto`; rerun leaves `test_rate.py::test_token_host_stands_on_its_own` and two `test_login.py` errors (zot not on the Windows PATH), identical on origin/main 6b8435c8; every hook test passes. **Q: gate the shell-form warm row (review Warn)?** Research: four bench runs, shell-form warm p50 2.71/2.66/1.96/1.50 ms against 2.65; it keeps the one `sh` the old chain had, so it sits at the old warm number by construction, while both cold rows pass by ~2× (chain 2.5–2.8, shell-form 3.9–4.1 ≤ 7.47). Decision: both cold rows and the chain warm row gate; shell-form warm is reported, not gated (ADR NFR amended). The original gate (chain only) passes on every run. Deferred (need an issue): live Windows runs of Copilot and Codex before admission; D2 payload-owned failure policy issue (never filed); D3 clean-Win11 Defender/SAC run; audit trail opened once per invocation (batching writer in `audit.rs`); PD-6 shields a composed deny but not `ask`; no-match path parses the whole table; phase-2 hooks spawn serially; Copilot fires Claude rows on Windows; CI check for `.dep-v0` in the embedded launcher; two missing WP-0 probes; shell-form warm latency unchanged for Claude, Qoder and Codex. Remote: verify-deep (Windows, macOS size and codesign) never dispatched. Convergence review (/hex-review medium over 0f0a7dc4..1901565f): all 4 R1 Blocks fixed, no new Block; Warns W1 control characters in launcher stderr, W2 unscreened launcher path in text output, W3 reader-spawn branch untested, W4 plan text (C-025, C-026, Design guarantee), W5 exit-65 doc row, and Suggests S1/S2 fixed in the second fix pass (3c812a0a, plan text here). Deferred: S3 multi-row status vs exit-65 case (documented split, untested). Gate 3 `task --force verify` green (4688 unit, 2159 acceptance passed); Windows hook subset green (8 passed, 45 platform skips) |
| 2026-10-26 | /hex-finalize. Run 1 pushed f04f4fb1 (7 commits, PR #246 draft); verify-deep Principle 9 red: the new root `[workspace]` listed `"."`, voiding `exclude`, so cargo claimed the p9 main checkout under `target/p9/src`. Fix (refinement cycle 1): root member implicit, `exclude = ["target"]`; local `task p9:diff` 0 differences. Run 2: release gate `task --force verify` green (2159 passed) on tree 569bbe03, recomposed, lease-pushed bbc2c107; verify-deep run 40342276353 12/12 success, 23/23 check runs on the head SHA success; PR flipped ready. Follow-ups filed: #247–#253. `/hex-retro --loop`: 22 rows created, report `.agents/retro/reports/2026-10-26.md`. |
