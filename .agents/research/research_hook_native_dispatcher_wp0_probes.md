# Research: WP-0 probes for the native grim-hook dispatcher

## Metadata

**Date:** 2026-10-26
**Domain:** build | client-hooks | Windows | packaging
**Status:** Probes run. Open items listed below.
**Source plan:** [plan_hook_native_dispatcher.md](../plans/plan_hook_native_dispatcher.md)
**ADR:** [adr_hook_native_dispatcher.md](../adr/adr_hook_native_dispatcher.md)
**Triggered by:** WP-0 of the plan. These probes settle PD-9 and the build and rename assumptions before WP-1.
**Expires:** 2027-04-26. Re-run the client probes when a harness changes its hook protocol.
**Companion artifacts:** [research_hook_dispatcher_ocx_shim_precedent.md](./research_hook_dispatcher_ocx_shim_precedent.md)

Method: scratch probes on host x86_64 Linux (rustc 1.95.0, dist 0.33.0), Windows 10.0.26300 on NTFS, Claude Code 2.1.310, Copilot CLI 1.0.91, Qoder 1.1.77. Repo left untouched.

## Summary

| PD-9 row | Verdict | Consequence |
|---|---|---|
| Claude exec form (native) | Holds on 2.1.310: argv verbatim, parent is `claude`, no shell. Floor 2.1.139 per changelog summary (unconfirmed against the changelog page). | None. |
| Copilot drops Claude `args` | Yes. Project `.claude/settings.json` and `settings.local.json` fire with argc=0. Global variant unprobed: no isolated way without editing the live home config. `CLAUDE_CONFIG_DIR` did not make Copilot fire the row. | Claude ships the C-N13 shell form (PD-9). |
| Qoder exec form | Unprobed. `qodercli` not logged in. | Qoder ships the shell form with Claude (decision recorded in the plan Progress Log). |
| Copilot own exec form | Holds on 1.0.91 (`.github/hooks/*.json`, `exec` + `args`, argv verbatim). Needs `COPILOT_ALLOW_ALL=true` for repo hooks in the probe. | Copilot ships exec form. |
| Windows single rename over a running exe | Fails, error 5 (ERROR_ACCESS_DENIED) on Windows 10.0.26300, NTFS. | Two-rename aside path (C-N5) is required. The O6 atomic-replace intent was withdrawn with O6 on 2026-10-25, so no owner escalation. |
| Nested build | Passes locally: host gnu (dist build), xwin msvc, zigbuild musl, llvm-cov, `clippy --workspace`. Offline nested build passes (`CARGO_NET_OFFLINE=true`, step 2, commit 662c2442). | macOS and native non-host dist runners need a CI run (push, then verify-deep or dist dispatch). |
| cargo-auditable forwarding (C-N2.2/.5) | dist injects auditable data via `RUSTC_WORKSPACE_WRAPPER`. Forwarding it gives the launcher a `.dep-v0` (~288 B). | `build.rs` forwards the wrapper only when it is cargo-auditable. `cargo audit bin` on the launcher is a P3 / C-024 CI step. cargo-audit is not installed locally. |
| Member SBOM | cargo-cyclonedx emits `grim-hook.cdx.xml`, but dist publishes only `grimoire.cdx.xml`. | Not shipped. Mitigations stay `.dep-v0`, `cargo audit bin`, and `grim hook list` sha256. |

## Std size floor (stub launcher, profile hook)

Bytes per target. Source: build probe 6. Stub is std + serde_json + `Command`, not the real dispatcher.

| Target | Tool | Bytes | Status |
|---|---|---|---|
| x86_64-unknown-linux-gnu | plain cargo | 325792 | measured |
| x86_64-unknown-linux-gnu | zigbuild | 347680 | measured |
| x86_64-unknown-linux-musl | zigbuild | 354584 | measured |
| aarch64-unknown-linux-gnu | zigbuild | 315472 | measured |
| aarch64-unknown-linux-musl | zigbuild | 330720 | measured |
| x86_64-pc-windows-msvc | cargo-xwin | 208896 | measured |
| aarch64-pc-windows-msvc | cargo-xwin | 193024 | measured |
| aarch64-apple-darwin | - | - | unprobed, needs CI |
| x86_64-apple-darwin | - | - | unprobed, needs CI |

Budget: 524288 B (512 KiB) for gnu targets; 655360 B (640 KiB) for musl.
The x86_64 gnu floor is 62% of budget. The musl floor is 54% of its budget.
This is a floor from a stub. The D1 measurement, recorded in P3, is the binding number.

## Probes

### Build

- **1a, dist build, host.** `dist build --artifacts=local --target x86_64-unknown-linux-gnu`. Passed, rc=0, 9m04s cold. Nested build ran under profile `hook`. Archive contains only `grim`.
- **1b, double compile.** `grim-hook` compiles as a dist-profile workspace member (discarded) and nested under `hook`. Cost: one extra small compile, no size or correctness impact.
- **1c, auditable mechanism.** cargo-auditable injects via `RUSTC_WORKSPACE_WRAPPER`, not `RUSTC_WRAPPER`. Our denylist strips it, so the nested launcher has no `.dep-v0`.
- **1d, forwarding in a scratch copy.** Removing the wrapper from the denylist gives the launcher a `.dep-v0` listing its exact dep tree (288 B compressed). Cost +360 B (326152 vs 325792). Survives `strip = symbols`.
- **1e, member SBOM.** `cargo cyclonedx -v` emits three files. The CI move step collects all three. `dist plan` lists only `grimoire.cdx.xml` as a release artifact.
- **1f, cargo audit bin.** Unprobed. cargo-audit is not installed. The extracted launcher carries its own `.dep-v0` only with forwarding (1d).
- **2, xwin.** `cargo xwin build --release --target=x86_64-pc-windows-msvc --locked`. Passed, rc=0, 2m51s. Launcher 208896 B. Five dead-code warnings in `src/install/hook_dispatch.rs` and `hook_launcher.rs` would fail a Windows `-D warnings` clippy.
- **3, zigbuild musl.** `cargo zigbuild --release --target x86_64-unknown-linux-musl --locked`. Passed, rc=0, 4m32s. Launcher 354584 B, static, stripped.
- **4, llvm-cov.** `cargo llvm-cov --workspace --no-report nextest -E 'test(build_embed)'`. 11/11 pass. Nested launcher has no `__llvm_prf` or covmap sections. Use `CARGO_TARGET_DIR`; `--target-dir` is not a valid llvm-cov flag.
- **5, clippy.** `cargo clippy --workspace --all-targets --locked -- -D warnings`. Passed, rc=0, 2m20s, no deadlock. The nested build ran under plain cargo, not clippy-driver.
- **6, std floor.** See the size section above.
- **7, offline nested build.** Pass: `touch build.rs; CARGO_NET_OFFLINE=true cargo build --locked` (step 2, 18 s).

### Windows rename

Victim: an exe printing its version, sleeping 8 s, then printing done. Replacer: `std::fs::rename` with `raw_os_error`.

- **A1, single rename over running image.** Fails, os error 5.
- **B1, rename running image aside.** `rename(path, .grim-hook.old.x.exe)` succeeds while running.
- **B2, rename new bytes into place.** `rename(stage, path)` succeeds after B1.
- **B3, fresh launch.** Prints the new version.
- **B4, old instance.** Runs to completion unaffected.
- **B5, delete `.old` while running.** Fails, error 5.
- **B6, delete `.old` after exit.** Succeeds.
- **ERROR_SHARING_VIOLATION (32).** Not observed. Not provoked deliberately.

Verdicts: single rename no; two-rename yes; delete of the aside copy only after exit.

### Client hooks

Logger: POSIX sh `argv-logger` in a scratch directory. Tokens: `00000000000000000000000000000000` and `1e10`. Events: SessionStart, UserPromptSubmit, PreToolUse. One early Claude run hung on API timeouts. Rerun was clean.

- **Claude 2.1.310, exec form.** Project `.claude/settings.json` with `args`. SessionStart, UserPromptSubmit, PreToolUse(Bash) all fired. argc=4, argv verbatim. Parent is `claude -p ...`, no `/bin/sh -c`. Docs say `command` is spawned directly with `args`, no shell. On Windows the exec form needs a real `.exe`.
- **Claude version floor.** The docs page gives no "added in" version. A changelog summary (secondary, not read on the official page) names 2.1.139 for the `args` field.
- **Claude shell-wrapper observation.** Earlier "ran through /bin/sh" was not reproduced for the `args` form. It likely came from the args-less shell form.
- **Copilot 1.0.91, own exec form.** `.github/hooks/probe.json` with `exec` + `args`. sessionStart and preToolUse fired, argc=4, argv verbatim. Parent is the copilot binary. Needed `COPILOT_ALLOW_ALL=true`. `copilot help config`: repo hooks live in `.github/hooks/*.json`.
- **Copilot running a Claude exec row.** Project `.claude/settings.json` and `settings.local.json` both fired. `args` dropped: argc=0. Stdin was non-empty. The global `~/.claude/settings.json` variant is unprobed. A scratch `CLAUDE_CONFIG_DIR` did not fire. Planning assumes global behaves like project.
- **Qoder 1.1.77.** `qodercli -p` returned "Not logged in". No hook event reached. Schema-only evidence stands.
- **Hygiene.** sha256 of `~/.claude/settings.json` and `~/.copilot/config.json` unchanged. Scratch hook configs removed.

## Open

- macOS builds, x86_64 and aarch64 apple-darwin. Needs CI with SDKROOT, MACOSX_DEPLOYMENT_TARGET and linker set.
- Native non-host dist runners: linux gnu x86_64 and aarch64, aarch64 musl, msvc x86_64 and aarch64, x86_64 musl. Needs push and a verify-deep or dist dispatch.
- `cargo audit bin` on the launcher artifact. cargo-audit not installed locally.
- Global `~/.claude/settings.json` under Copilot. No isolated test yet.
- Qoder live probe. Needs a login.
- `ERROR_SHARING_VIOLATION` (32). Not provoked. The Defender race was not reproduced.
- Claude exec-form floor 2.1.139. Confirm against the official changelog page.
- Member SBOM publication. Not wired into the dist manifest.

## Execution results (2026-10-26)

- **Launcher size, D1 closed locally** (profile `hook`): gnu with `cargo auditable` 506,296 B (budget 524,288), musl 610,976 B (655,360), msvc via xwin 346,112 B (524,288). The gnu PE/ELF carries `.dep-v0`. macOS is unmeasured until verify-deep's `hook-size-macos` runs.
- **`/DEPENDENTLOADFLAG:0x800`**: both xwin-built PEs carry `DependentLoadFlags = 0x800`.
- **`ERROR_SHARING_VIOLATION` provoked**: a handle held without share-delete made the rename fail with 5 and the aside rename with 32; the retry now covers the aside step. Replace-by-rename of a running launcher works and the `.old` is reaped on the next install.
- **icacls (C-N14): not applied.** A dir under `%LOCALAPPDATA%` inherits only SYSTEM, Administrators and the user; `grim-home-shared` already confines the home to the profile; `/inheritance:r` without re-grants locks the user out.
- **Windows clients**: Claude verified live (`claude -p` fired the registered row through Git Bash and PowerShell, four home shapes). Copilot and Codex were exercised only by emulating their launch (CreateProcess of `exec`+`args`; `cmd.exe /C`): neither CLI is installed on the host. Qoder unobserved.
- **Clean-Win11 first run (D3)**: not run, no clean VM.

## Search terms

`RUSTC_WORKSPACE_WRAPPER cargo-auditable`
`cargo auditable .dep-v0 forwarding`
`Windows rename running exe os error 5`
`MoveFileEx rename aside running image`
`Claude Code hooks args exec form`
`Copilot CLI hooks exec args .github/hooks`
`cargo-xwin msvc cross build embed`
`cargo cyclonedx member SBOM dist`
