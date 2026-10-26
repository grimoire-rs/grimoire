# Research: ocx's embedded native shim as precedent for a `grim-hook` dispatcher binary

## Metadata

**Date:** 2026-10-25
**Domain:** packaging | build | security | Windows
**Triggered by:** `adr-hook-native-dispatcher` ADR work. The owner states that the sibling project ocx
(`/home/mherwig/dev/ocx`, branch `goat`) already solved "small native binary embedded in the main binary and
written to disk". This file extracts that precedent and says what ports to grimoire.
**Expires:** 2027-04-25 (ocx's shim build chain moved three times in May 2026; re-read `build-windows-shims.yml`
and `crates/ocx_store/src/shim.rs` before relying on a mechanism below)
**Companion artifacts:** [`research_hooks_trampoline.md`](./research_hooks_trampoline.md),
[`research_hooks_hotpath_cost.md`](./research_hooks_hotpath_cost.md)

Method: read the ocx source and its `.claude/artifacts/` ADRs directly; one local measurement (section 2). Nothing
here was run on Windows. Citations are `ocx:<path>:<line>`. Where an ocx ADR and the shipped code disagree, the
code wins and the drift is named.

## Direct Answer

1. **ocx embeds a prebuilt, committed blob, not a build-time artifact.** `include_bytes!` of
   `crates/ocx_store/src/shims/ocx-shim-{x86_64,aarch64}.exe`, no `build.rs`, arch-gated, empty slice off Windows
   (`ocx:crates/ocx_store/src/shim.rs:16-34`). A separate workflow cross-builds the blob hermetically on Linux with
   `cargo-zigbuild` + pinned Zig, and a refresh PR re-cuts the committed bytes.
2. **The precedent is narrower than it sounds.** ocx has exactly two blobs, both Windows, both built by one
   cross-toolchain, and it solves a *launcher* problem (spawn-and-wait, ~475 lines of pure logic). It has no Unix
   native shim at all (Unix launchers are `.sh`). Grimoire needs a dispatcher on Linux, macOS and Windows, so the
   blob count, the toolchain story and the size budget all differ.
3. **The parts that transfer cleanly are the operational ones:** content-addressed published file, hardlinks for
   multiple consumers, write-temp-then-persist with Windows retry, a length check as self-heal, a recency CI gate,
   and the "no byte-reproducibility, so gate on provenance" decision.
4. **The size budget is the main risk.** A local probe of `std` + `serde_json` + `Command` with ocx's `shim`
   profile is 366 KiB on glibc and 453 KiB on musl, before any dispatcher logic. The 500 KB budget is tight to
   infeasible on musl (section 2).

## 1. Build and embedding mechanism

| Aspect | ocx choice | Cite |
|---|---|---|
| Crate | `crates/ocx_shim`, `[[bin]] ocx-shim`, `publish = false`, "Internal blob (include_bytes!'d into ocx)" | `ocx:crates/ocx_shim/Cargo.toml:7-12` |
| Dependencies | `windows-sys` + `dunce` only, Windows-gated; nothing internal, `ocx_store` does not depend on the crate | `ocx:crates/ocx_shim/Cargo.toml:14-21` |
| Embed | `include_bytes!` in `ocx_store/src/shim.rs`, path relative to the source file; `SHIM_BYTES = &[]` and `SHIM_SHA256 = ""` off Windows | `ocx:crates/ocx_store/src/shim.rs:16-34` |
| No `build.rs` for the blob | The only ocx `build.rs` is `ocx_cli/build.rs` (vergen provenance), unrelated | `ocx:crates/ocx_cli/build.rs:1-10` |
| Profile | dedicated `[profile.shim]` (opt-level `z`, `lto = true`, `codegen-units = 1`, `panic = "abort"`, `strip = "symbols"`), because Cargo takes profiles only at the workspace root and a per-crate `release` override is impossible | `ocx:Cargo.toml:47-61` |
| Build command | `task rust:shim:build TARGET=<arch>-pc-windows-gnullvm` runs `cargo zigbuild -p ocx_shim --profile shim --locked --target $TARGET` | `ocx:taskfiles/rust.taskfile.yml:555-606` |
| Targets | `x86_64-pc-windows-gnullvm`, `aarch64-pc-windows-gnullvm`, pinned in `rust-toolchain.toml` (channel 1.95.0) | `ocx:rust-toolchain.toml:1-9` |
| Pins | Zig 0.16.0 by SHA-256-checked tarball from ziglang.org; `cargo-zigbuild@0.22.3`; both deliberately outside any package manager | `ocx:.github/workflows/build-windows-shims.yml:148-166`, `ocx:crates/ocx_store/src/shim.rs:10-11` |
| Decoupled from the main build | cargo-dist builds `ocx` for `*-pc-windows-msvc`; the gnullvm blob is just bytes the msvc build embeds. `dist-workspace.toml` knows nothing about the shim | `ocx:dist-workspace.toml:12` |
| Workspace hygiene | `ocx_shim` is a normal workspace member, so `cargo check --workspace` compiles it on Linux: `#![cfg_attr(not(windows), allow(dead_code))]` plus a non-Windows `run()` stub that is `unimplemented!()` | `ocx:crates/ocx_shim/src/main.rs:12`, `ocx:crates/ocx_shim/src/main.rs:562-566` |
| Host-testable core | Win32 calls are split from pure logic in `core.rs` (stem, sidecar grammars, argv quoting, exit mapping) so Linux CI runs the spec suite | `ocx:crates/ocx_shim/src/core.rs:4`, `ocx:.github/workflows/build-windows-shims.yml:56-89` |
| Windows cfg check | `cargo check -p ocx_shim --target {x86_64,aarch64}-pc-windows-msvc` in `rust:verify`; whole-workspace Windows check is a separate xwin Docker task | `ocx:taskfiles/rust.taskfile.yml:159-160` |
| Bazel | `crates/ocx_shim/BUILD.bazel` is a parallel description; CI recency gate excludes it explicitly because it cannot change blob bytes | `ocx:.github/workflows/build-windows-shims.yml:272-310` |

### Why committed blob (ADR Axis A)

`ocx:.claude/artifacts/adr_windows_exe_shim.md:140-171`. Chosen A1 (committed blob + `include_bytes!`). Rejected:

- **A2 `build.rs` cross-compiles the shim.** Needs a Windows cross toolchain on every contributor laptop, "fragile,
  slow, breaks `cargo check` ergonomics", and cannot reconcile with a CI signing step (chicken-and-egg).
- **A3 CI downloads the shim as a release artifact at build time.** Network dependency, breaks offline and hermetic
  builds, bootstrapping paradox.

Stated precedent: uv (`uv-trampoline/trampolines/*.exe`) and pixi (`build-trampoline`) commit prebuilt trampolines
(`ocx:.claude/artifacts/research_windows_shim_patterns.md:10-18`). ocx records this as its one tech-strategy deviation.

### Why zigbuild / gnullvm (hermetic ADR)

`ocx:.claude/artifacts/adr_shim_hermetic_zigbuild.md`. The history is the useful part, three pivots in two days:

1. **cargo-xwin + msvc.** Blob ~140 KiB, but the MSVC SDK/CRT floated, so a blob refreshed on a dev box failed CI's
   byte-equality rebuild ("refresh drifted blobs" treadmill). Pinning cargo-xwin + SDK + major was verified
   insufficient (lines 14-15).
2. **cargo-zigbuild + `x86_64-pc-windows-gnu`.** Hermetic, but gnu had no aarch64 variant and was 280 KiB. aarch64
   `gnullvm` failed to link: `___chkstk_ms` / `__chkstk` undefined under `-nolibc` (lines 52-58).
3. **gnullvm + inline stack-probe builtins.** `chkstk.rs` supplies libgcc `___chkstk_ms` (x86_64) and compiler-rt
   `__chkstk` (aarch64) via `global_asm!`, "upstream verbatim", cfg-gated on `target_abi = "llvm"`
   (`ocx:crates/ocx_shim/src/chkstk.rs:1-51`, `ocx:crates/ocx_shim/src/main.rs:16-18`).

**Byte-reproducibility was abandoned** (addendum 2, lines 62-64): the gnullvm PE link is nondeterministic run to run
even with pinned Zig, pinned rustc and `/Brepro` (lld embeds a per-link build id; ~10 CI runs). The taskfile records a
sharp edge: `RUSTFLAGS` must be assigned on the command line, because go-task `env:` loses to an inherited variable
and CI exports `RUSTFLAGS=-D warnings`, which silently made `/Brepro` inert for every shipped blob
(`ocx:taskfiles/rust.taskfile.yml:593-606`). Another: a dev box with `rust-src` installed bakes `$HOME/.rustup/...`
paths into the blob through `core::panic::Location`; fixed with `--remap-path-prefix` back onto `/rustc/<hash>`
(lines 563-575).

### CI integration (`build-windows-shims.yml`)

- Triggers on push and PR to main, **path-filtered** to the build inputs: `crates/ocx_shim/**`, the blobs,
  `shim.rs`, `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, the taskfile, the workflow, and the shim tests
  (`:13-43`).
- `gate` (Linux): nextest of the host-runnable shim and launcher specs. Matrix `build` and `acceptance-windows`
  both `needs: gate`, so Windows minutes are only spent when the cheap suite is green (`:56-97`, `:318`).
- `build` per target: install pinned Zig + cargo-zigbuild, run the task, **upload the fresh blob as
  `ocx-shim-fresh-<target>` before validation and even on failure** so the refresh PR can use it (a dev box may not
  be able to install the pinned Zig) (`:168-195`).
- **Validate the committed blob** (not the fresh one): MZ magic, size <= `SHIM_SIZE_BUDGET` (grepped out of
  `shim.rs`), and SHA-256 equals the recorded constant (`:208-243`).
- **SLSA build provenance** via `actions/attest-build-provenance` on the committed blob (`:244-252`). The in-tree
  SHA is called "a corruption canary, not a provenance control" (`shim.rs:20`).
- **Recency gate** (`:272-310`): the last commit touching the shim sources must be an ancestor of the last commit
  touching the blobs. Added after a real incident: the `.exec` sidecar grew a second line in source while the
  committed blobs kept the one-line parser, "which made every Windows trampoline exit 78 with a fully green tree".
  Neither the SHA canary nor attestation can see "blob older than source".

## 2. Size

| Fact | Value | Cite |
|---|---|---|
| Shipped blobs today | 333 KiB x86_64, 239 KiB aarch64 (`ls` of the committed files; docs say "about 333 KiB / 239 KiB") | `ocx:crates/ocx_store/src/shims/`, `ocx:website/src/content/docs/guides/stable-paths.mdx:70` |
| Budget | `SHIM_SIZE_BUDGET = 512 * 1024`, enforced three ways: compile-time `const _: () = assert!`, a unit test, and the CI grep | `ocx:crates/ocx_store/src/shim.rs:14`, `:52-56`, `ocx:.github/workflows/build-windows-shims.yml:219-228` |
| History | initial estimate "<80 KB" -> xwin/msvc ~128-138 KiB (budget 256) -> gnu 280 KiB (budget 512) -> `-Zbuild-std` PoC 64-66 KB -> **build-std dropped**, shipping 239-333 KiB | `adr_windows_exe_shim.md:301`, `adr_shim_hermetic_zigbuild.md:52-64`, `taskfiles/rust.taskfile.yml:588-592` |
| Why build-std was dropped | not reproducible and forces a nightly bootstrap (`RUSTC_BOOTSTRAP` on pinned stable); the saving was shedding std's backtrace stack (gimli / addr2line / object / miniz), "the actual bloat, not user deps" | `adr_shim_hermetic_zigbuild.md:56`, `:62`, `:92` |
| `no_std` | rejected: reaches ~20-30 KB but needs a custom allocator, "NOT worth complexity". `std` + `windows-sys` is the choice | `research_windows_shim_tech.md:25-27`, `adr_windows_exe_shim.md:111-113` |
| Other techniques | `windows-sys` over `winapi`; `panic = "abort"` with every fallible path returning an exit code; no Tokio (sync `std`, one child, blocking wait); no `ocx_env` edge ("the shim's size budget", two `#[expect(clippy::disallowed_methods)]` for raw `std::env::var_os`) | `ocx:crates/ocx_shim/src/main.rs:239-241`, `:273-276` |

**Grimoire measurement (this research, not from ocx).** A probe crate reading JSON from stdin with `serde_json`,
spawning one `std::process::Command` and printing a JSON line, built with ocx's exact `[profile.shim]`:

| Target (rustc 1.93.1, prebuilt std, no build-std) | Stripped size |
|---|---|
| `x86_64-unknown-linux-gnu` | 374,624 B (366 KiB) |
| `x86_64-unknown-linux-musl` | 463,408 B (453 KiB) |

Caveats: rustc 1.93.1 (not 1.95), no dispatch logic, no glob/timeout/JSONL code, macOS and Windows not measured.
It matches ocx's own finding that the prebuilt std (backtrace + panic machinery) dominates. Consequence: a 500 KB
budget for `std + serde_json` is **not safe on musl** and unproven on macOS and Windows; the ocx mitigation
(`build-std` + `panic_immediate_abort`) buys back ~270 KB but costs nightly and reproducibility, and ocx rejected it.
Cheaper levers to measure before committing a number: avoid `serde_json::Value` in favour of a typed minimal parse
or a hand parser, `-Cpanic=abort` is already on, `Command` pulls in a lot of unwind-free code but little else.

## 3. How the blob reaches disk and gets updated

ocx never replaces a running executable. It avoids the problem by construction.

- **One content-addressed published file.** `ShimBinStore` publishes the embedded bytes once to
  `$OCX_HOME/.bin/ocx-shim/<sha256-hex>.exe`. The name is the digest of `SHIM_BYTES` computed at runtime
  (`debug_assert`ed against the recorded constant) (`ocx:crates/ocx_store/src/file_structure/shim_bin_store.rs:1-24`,
  path layout `:90-105`). A new ocx with a new blob therefore writes a **new file**; the superseded blob stays as
  accepted litter ("no GC", adr_lazy D4, a handful of ~300 KB files, bounded by ocx versions x arch)
  (`ocx:.claude/artifacts/adr_lazy_package_loading.md:317-330`).
- **Multiple consumers share one inode.** Every entrypoint `<name>.exe` is a hardlink to that file
  (`ocx_store::hardlink::create`), not a copy: "one inode, one signature, one Defender scan"
  (`ocx:crates/ocx_package_manager/src/launcher/generate.rs:131-143`, ADR `adr_lazy_package_loading.md:293-315`,
  test `generate_hardlinks_every_exe_to_the_one_shared_store_blob`, `generate.rs:~586`). **No copy fallback**: a
  cross-device `$OCX_HOME` surfaces `CrossesDevices` (exit 74), on the argument that on such a home no package can
  be installed anyway (D3).
- **Atomic publish.** Stage `tempfile::NamedTempFile` in the store root, `write_all`, `sync_data`, then persist
  (`shim_bin_store.rs:140-175`). `persist_temp_file_if_absent` (no-clobber) for the normal case, because "a
  replacing racer orphans what hardlinks the winner's file"; plain `persist_temp_file` (replace) only over a torn
  blob (`shim_bin_store.rs:48-80`, `ocx:crates/ocx_util/src/fs.rs:126-135`).
- **Windows transient retry.** `ERROR_ACCESS_DENIED` (5) and `ERROR_SHARING_VIOLATION` (32) are retried at
  0/100/400/800 ms with +-25% jitter, because Defender opens freshly written files
  (`ocx_util/src/fs.rs:60-90`, `:196-229`). The same helper exists for `rename`.
- **Self-heal.** `ensure()` is idempotent and lock-free. Pre-check is **length, not existence**
  (`published_blob_is_intact`, `shim.rs:48-50`): a crash can leave a torn blob that every hardlinked launcher would
  run, so a wrong-length file is republished over (`OverTornBlob`). A lost publish race converges on the winner's
  file (`shim_bin_store.rs:106-170`). A test seam (`__OCX_TESTING_SHIM_LOST_PUBLISH_RACE`) forces the lost-race leg
  because no black-box input reaches it. Length is a weak check (equal-length corruption passes); the SHA canary is
  only on the embedded constant, not re-verified against the published file.
- **Write ordering for the pair.** `.exe` hardlink first, `.shim` sidecar second, in a single task: a split could
  leave a sidecar without its exe, "the worse partial state"; exe-without-sidecar is recoverable (exit 78 with a
  regenerate hint) (`generate.rs:131-143`, ADR `:504-507`).
- **No in-place overwrite.** The adr_lazy ADR notes that all N names share one inode, so a *running* `<name>.exe`
  locks the inode behind every sibling name; regeneration therefore goes through `persist_temp_file`'s retry rather
  than in-place overwrite (`adr_lazy_package_loading.md:309-312`). In practice packages are staged into a temp dir
  and published by one atomic rename, so the launcher slot is never occupied and `hardlink::create` (fails if
  present) is used, not `update` (`adr_windows_exe_shim.md:789-792`). `hardlink::update` (remove then link) exists
  but is not on this path (`ocx:crates/ocx_store/src/hardlink.rs:36-44`).
- **Per-install state in sidecars, never in the binary.** `SHIM_BYTES` is byte-identical for every entrypoint so one
  Authenticode signature and one embed serve all. Rejected: PE-patching the pkg_root into a per-install copy (B3,
  invalidates the signature), encoding it in the filename (B2, not representable)
  (`adr_windows_exe_shim.md:173-200`).

### Version and identity stamping, downgrade protection

**ocx has none, deliberately.** The blob carries no version string, no embedded identity and no `VERSIONINFO`.
Identity is the content digest in the published file name. There is no downgrade protection because nothing is ever
replaced: an older ocx hardlinks its older digest-named blob, a newer ocx its newer one, and both coexist. The
accepted result is a mixed estate that heals when `install`/`select` re-runs `generate()` ("pre-1.0, no migration
code", `adr_windows_exe_shim.md:644-655`).

- A canary asserts the blob has **no** Win32 `VERSIONINFO` resource (`contains_version_resource`, scanning for the
  UTF-16LE `VS_VERSION_INFO` / `StringFileInfo`), because one `OriginalFilename` would contradict every hardlinked
  name (MITRE T1036.005) (`shim.rs:36-45`). Note this conflicts with ADR Phase-2 item 4 ("PE product/version
  metadata populated"); the shipped canary wins.
- **The skew risk is on the wire format, not the file.** The reader (blob) and writer (ocx) of the sidecar can
  disagree after a source change without a blob refresh. The paired-golden rule plus the CI recency gate are the
  controls: `ocx_shim::tests::shim_wire_token_matches_sh_body` is bound to `body.rs::tests::launcher_wire_token_is_bound_to_shim_producer`
  (`adr_lazy_package_loading.md:279-284`). Sidecar grammar was designed as a One-Way Door: UTF-8 no BOM, one line,
  <= 32 KiB, forbidden `' " \n \r \0`, only *widening* the admissible set is allowed
  (`adr_windows_exe_shim.md:560-596`).
- `design_launcher_patch_identity.md` is **not** about binary identity. It is about which package names a launcher
  matches against patch rules (a forwarded `OCX_LAUNCH_IDENTITIES` env map). Not relevant to grimoire's blob.

## 4. Windows specifics

**Invocation.** The shim is a console-subsystem exe reached by bare name through default `PATHEXT` (`.EXE` is
unconditionally in it); Git-Bash resolves `<name>.exe` directly and never consults PATHEXT. The `.cmd` launcher was
removed outright, not kept as a fallback, because an orphan `.cmd` leaves the `%*` BatBadBut / CVE-2024-24576
re-parse reachable by explicit extension (Axis C2, `adr_windows_exe_shim.md:202-237`). Acceptance tests run on
`windows-latest` through cmd, pwsh and Git-Bash (`test/tests/test_windows_shim.py:203-215`); bare `bash` there is
the WSL launcher and exits 1 without running the shim, so the test resolves Git-for-Windows bash explicitly
(`adr_shim_hermetic_zigbuild.md:66-68`). A no-console parent case is tested (`test_shim_runs_without_console`).

**argv and quoting.** The shim spawns with `CreateProcessW` directly, never through `cmd.exe`. It rebuilds the
command line with `CommandLineToArgvW`-compatible quoting (`append_quoted_arg`: quote on empty, space, tab, `"` or
any ASCII control byte; double backslashes before a quote and at the end) and takes `argv[1..]` verbatim
(`ocx:crates/ocx_shim/src/core.rs:431-466`). A pinned/baked program goes in `lpApplicationName` so
`C:\Program Files\...` is not parsed as `C:\Program.exe` (CWE-428) (`main.rs:~228-232`). Tests cover spaces, `&`,
Unicode, `%` and `"`.

**Exit codes.** Child's code is propagated as the full i32 (`GetExitCodeProcess` -> `exit`), including
`0xC0000005`-style NT status codes; the shim never remaps. Its own failures use sysexits-aligned codes:
78 config (sidecar missing/malformed), 77 permission (containment, `ERROR_ACCESS_DENIED`), 74 I/O, 69 `ocx` not
found; one lowercase `ocx-shim: ...` stderr line, no trailing period (`main.rs:21-96`, ADR `:343-352`). 64/65 are
unused because argument parsing is the main binary's job.

**Process hygiene** (all worth copying for a hook runner that spawns payloads):
- no-op `SetConsoleCtrlHandler` installed **before** `CreateProcessW` so a Ctrl+C cannot kill the shim before it
  waits; never `CREATE_NEW_PROCESS_GROUP` (`main.rs:544-560`, ADR Contract 1 step 7);
- job object with `KILL_ON_JOB_CLOSE`, child born inside it through `PROC_THREAD_ATTRIBUTE_JOB_LIST` (no
  suspend-assign-resume race); best-effort, degrade to plain spawn with a stderr line (E7);
- `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` whitelisting only the de-duplicated valid std handles (CWE-403); three
  documented degraded modes including no-console;
- `SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32)` before any other Win32 call against DLL planting beside
  the shim, with no `SetDllDirectoryW(NULL)` fallback (`main.rs:108-133`);
- `GetModuleFileNameW` with a doubling buffer (truncation silently picks the wrong sidecar), `dunce::canonicalize`
  because `\\?\` paths break `CreateProcessW`.

**Signing, SmartScreen, Defender.** Authenticode is **not done**: the blobs ship **unsigned**
(`ocx:website/src/content/docs/guides/stable-paths.mdx:70`, workflow header `build-windows-shims.yml:6-10`).
Research chose SignPath Foundation (free OSS, Linux-CI compatible, GitHub-OIDC) as Phase 2, with Azure Trusted
Signing as fallback only for a US/CA/EU/UK legal entity, EV certs "no SmartScreen edge post-2024"
(`research_windows_shim_signing.md:15-27`). Mechanics worth keeping: the Authenticode hash excludes CheckSum, the
cert-table directory entry and the cert table, so `include_bytes!` -> verbatim write preserves a signature;
**strip before sign, never after**; RFC 3161 timestamp mandatory. Known accepted negative: AppLocker / WDAC / Smart
App Control environments that block unsigned exes have **no launcher at all**, the user doc points at execution
policy / AppLocker / antivirus for exit 77 (`website/.../entry-points.md:181`). There are **no empirical SmartScreen
or Defender measurements in ocx**; the only Defender evidence is operational (transient sharing violations on
freshly written files, `adr_file_lock_unification.md:514`) and the one-scan-per-inode argument for hardlinks.

## 5. Runtime behavior, config location, failure policy

- **Self-location, not env.** The shim finds its own path with `GetModuleFileNameW`, strips one trailing `.exe`
  (case-insensitive) to get the entrypoint **stem**, and reads a sibling sidecar `<dir>/<stem>.{shim,shimref,exec}`
  probed in that order (`core.rs:9-31`, `:95-100`, `main.rs:150-205`). Per-invocation state is the sidecar; the
  binary is constant. A read error other than `NotFound` fails rather than falling through, "or a broken `.shim`
  silently becomes a package materialization" (`main.rs:~190-200`).
- **Env it consults:** `OCX_BINARY_PIN` (defined, even empty, selects the pin; mirrors the old `.cmd`
  `IF DEFINED`), `OCX_HOME` (optional containment check, delegated if unreadable), strips `OCX_GLOBAL` /
  `OCX_PROJECT` for `.exec` trampolines (`main.rs:233-262`).
- **Defense in depth, not the trust boundary.** The sidecar is explicitly "not itself a trust boundary"; the
  authoritative check (`validate_package_root`, and for deferred tools the digest-verified fetch) lives in the main
  binary. Write access to the shims tree is declared equal to write access to `packages/`
  (`adr_windows_exe_shim.md:571`, `adr_lazy_package_loading.md:270-278`).
- **Failure policy:** fail closed with a named exit code for config problems (78), containment (77), spawn (74/77),
  missing `ocx` (69); fail **open and logged** for hardening that is not correctness (job object, handle list,
  attribute list: E7). `panic = "abort"` with no panicking path in release.
- **The wire ABI is a One-Way Door guarded by paired goldens** on both producers (the `.sh` body and the shim
  const), added to a named canary rule (`adr_lazy_package_loading.md:279-284`).

## 6. Rejected alternatives (ocx)

| Alternative | Why rejected | Cite |
|---|---|---|
| Keep `.cmd` launchers (even hardened) | `%*` re-parse = BatBadBut CVSS 10 class; `DisableDelayedExpansion` is only interim | `adr_windows_exe_shim.md:39-43` |
| Coexist `.exe` + `.cmd` (C1) | orphan `.cmd` reachable by explicit extension keeps the vuln; keeps the PATHEXT inject/warn subsystem alive | `:204-236` |
| Flag-gated `.exe` (C3) | "needless complexity" for a CVE fix | `:220-225` |
| `build.rs` cross-compile (A2) / CI-downloaded artifact (A3) | see section 1 | `:153-171` |
| pkg_root in filename (B2) / PE-patched per-install copy (B3) | not representable / breaks Authenticode | `:186-199` |
| Scoop-style direct target exec | loses `launcher exec` clean-env resolution indirection | `:52-54` |
| `no_std` + custom allocator | 20-30 KB but complexity | `research_windows_shim_tech.md:25-27` |
| cargo-xwin pinning | floating MS manifest, verified insufficient | `adr_shim_hermetic_zigbuild.md:14` |
| Byte-equality CI gate | gnullvm link nondeterministic | `:62-64` |
| `-Zbuild-std` + `panic_immediate_abort` | non-reproducible, nightly bootstrap | `rust.taskfile.yml:588-592` |
| Copy shim per entrypoint | 235-329 KB each, dominant cost under lazy loading; replaced by hardlink | `adr_lazy_package_loading.md:31-36` |
| Copy fallback when hardlink fails | nothing else works on such a home either | `:293-308` |
| GC of `.bin/ocx-shim` | bounded set of regenerable files; `nlink==1` race could collect a live entry | `:317-330` |
| mise/Volta-style symlink to the main binary + argv0 dispatch | prior art only; ocx chose a dedicated small shim whose job is to re-enter `ocx`. Not argued as a rejected option in an ADR | `research_lazy_shim_prior_art.md:64,:75` |

## 7. Transfers directly vs differs

### Transfers directly

1. **Content-addressed publish + atomic persist.** `$GRIM_HOME/hooks/bin/<sha256>.<ext>`-style file written via
   temp file + `sync_data` + no-clobber persist, Windows 5/32 retry with jitter, length-checked self-heal, lost-race
   converges on winner. Directly maps to a dispatcher that "all workspaces share".
2. **Replace-by-new-name instead of replace-in-place.** Solves "Windows cannot overwrite a running exe" and
   downgrade/upgrade ordering with zero code. Only the *registered* path must be stable (see below).
3. **No version stamping inside the binary.** Digest-in-filename is the identity; same stance suits a frozen-contract
   (Principle 9) project. Keep any `--version` output out of the identity decision.
4. **Committed prebuilt blob + `include_bytes!`, empty slice where unused**, plus the three guards: size
   `const assert`, SHA canary, and the **recency gate** (the incident it came from is exactly the failure mode of a
   dispatcher whose dispatch-table grammar is read by one binary and written by another).
5. **Provenance-not-reproducibility gate** (path-filtered workflow, upload fresh artifact first, SLSA-attest the
   committed bytes). Applies as soon as any blob is cross-linked rather than natively built.
6. **Pure-core / thin-OS split** so Linux CI covers parsing, matching, quoting and exit mapping.
7. **Process hygiene on Windows** if the dispatcher spawns payloads: console ctrl handler before spawn, job object
   born-in, handle-list whitelist, `SetDefaultDllDirectories`, `CreateProcessW` with `lpApplicationName`, exact
   `CommandLineToArgvW` quoting, full-i32 exit passthrough, no `cmd.exe` in the chain.
8. **Wire/grammar compatibility discipline:** paired goldens across producer and reader, widen-only grammar.
9. **Windows test matrix:** cmd, pwsh and Git-Bash (explicit path), no-console parent, missing/malformed config.
10. **Unsigned-at-first with a documented signing follow-on**, and the signing rules (strip before sign, RFC 3161).

### Differs, so do not port blindly

| Dimension | ocx shim | grim-hook | Consequence |
|---|---|---|---|
| Platforms | Windows only (2 arches). Unix uses `.sh`, no native binary | Linux (glibc/musl x86/arm), macOS (x86/arm), Windows (x86/arm) | Up to 8 targets. ocx's "one Linux cross-toolchain emits all blobs" works for 2 PE files; for 8 you need Zig for all, or native runners, or abandon the blob (below) |
| Job | spawn one fixed child, wait, forward exit code; ~475 lines of logic | read JSON table, match tool calls, spawn **many** payloads with timeouts, JSON stdin/stdout, audit JSONL, per-client response projection | No `no_std`; needs `serde_json`; more std surface (threads/timeouts, file append). ocx's size evidence does not transfer |
| Size | 239-333 KiB, budget 512 KiB, no serde | budget <= 500 KB with `std + serde_json` | Probe says 366 KiB (gnu) / 453 KiB (musl) before logic; see section 2 |
| Per-invocation state | sidecar file next to a hardlinked exe named after the entrypoint (many names, one inode) | one path `<abs>/hooks/grim-hook <client> <event> <token>` registered in harness settings, state in `dispatch.json` | Hardlink-per-name machinery is unnecessary; one published file suffices. argv carries `<client> <event> <token>` where ocx used the file stem |
| Path stability | irrelevant: launchers are regenerated next to each package | the **registered path is in third-party settings files** and must stay valid across upgrades | Digest-named blobs need a stable indirection: either keep `hooks/grim-hook` as the stable name and replace it (Windows running-exe problem returns), or a tiny stable name that hardlinks/rename-swaps to `bin/<sha>`; or register the digest path and rewrite settings on upgrade (touches client config, bigger blast radius). ocx offers no precedent for stable-path replacement |
| Replacing a running exe | avoided by construction | unavoidable at the stable path; Windows allows rename-aside of a running exe (`MoveFileEx`/rename to `.old`) but ocx does not do it | Needs its own design; ocx's retry helper handles Defender, not image-section locks |
| Consumers | thousands of per-entrypoint names | every workspace shares one | Same sharing idea, much simpler layout |
| Trust | sidecar is explicitly not a trust boundary; main binary re-validates | dispatcher **is** the executing boundary: it reads the table and runs hooks, gated by consent records | ocx's "delegate containment to the main binary" has no analogue; the dispatcher must verify consent/GRIM_HOME rules itself (see `adr_hook_workspace_consent.md`) |
| Build toolchain | main binary msvc, blob gnullvm; blob decoupled | main binary built by cargo-dist per target; blob must match target OS/arch | A Linux blob could be built natively by the same job as the main binary and embedded via a two-stage build, but that is the `build.rs`/ordering path ocx rejected. Committed blobs for 8 targets multiplies git weight (~0.4 MB each) and refresh PRs |
| Signing | none yet | macOS Gatekeeper/quarantine and notarization for a file grim *writes itself* (not downloaded, so no quarantine xattr); Windows as ocx | macOS ad-hoc signing on write (arm64 requires a signature) is a new problem ocx never faced; not researched here |
| Failure policy | exit 78/77/74/69, stderr line | must obey each client's hook protocol (exit 2 blocks in Claude, etc.) | ocx's sysexits mapping must not leak into hook decisions |

### Option that falls out of the comparison (for the ADR, not decided here)

ocx's own prior-art table shows the other family: **mise and Volta use one shared main binary with argv0 dispatch**
(`research_lazy_shim_prior_art.md:64`, `research_shell_env_reconciler_and_launcher_farm.md:25,:53`). For grimoire
that is "register `grim hook run` and skip the embedded blob entirely", which `research_hooks_hotpath_cost.md`
already analyses as the startup-cost question. The ocx precedent only pays for itself when startup cost or a
dependency-free runtime justifies a *second binary*; ocx justified it by security (no `cmd.exe`) and by not
starting the full ocx runtime per call. A measured startup comparison of `grim hook run` vs a minimal blob is the
missing datum, and ocx has none to lend.

## Gaps and unverified

- **Not run on Windows.** All Windows behavior is from source reading and ocx's own tests.
- ocx ADR `adr_windows_exe_shim.md` is partly stale vs code: it names `crates/ocx_lib/src/shims/` and a 256 KiB
  budget; the code uses `crates/ocx_store/src/shims/` and 512 KiB. It also lists "no `.cmd`" while its
  `system_design_windows_exe_shim.md` still describes the coexist (C1) design. Trust code and the hermetic ADR.
- No ocx data on SmartScreen reputation, Defender false positives, or startup latency of the shim.
- macOS: ocx has no precedent for writing an executable to disk and having Gatekeeper accept it.
- The size probe used rustc 1.93.1 and a toy payload; re-measure with the real dispatcher on the pinned toolchain.

## Search terms

`cargo-zigbuild gnullvm chkstk`, `include_bytes! prebuilt trampoline uv pixi`, `hermetic-launcher`,
`persist_noclobber hardlink shared inode`, `CreateProcessW PROC_THREAD_ATTRIBUTE_JOB_LIST`,
`SetDefaultDllDirectories LOAD_LIBRARY_SEARCH_SYSTEM32`, `Authenticode strip before sign`, `SignPath Foundation`.
