# Research: Windows-only test skips

## Metadata

**Date:** 2026-11-10
**Domain:** testing
**Triggered by:** issue #351 (Windows test hygiene left after the issue sweep): the suite skips 562 tests on Windows against 189 on Linux and no list existed
**Expires:** 2027-02-10 (skip reasons track the hook launcher and settings work; re-run after either changes)

## Direct Answer

Method: the Linux set is the junit report of a full `task verify` run at grim commit `6081689e` plus the #351 working tree (3,277 passed, 189 skipped, 0 failed). The Windows set is the `deep-tests-Windows` junit artifact of the latest `verify-deep.yml` run on `hex/issue-sweep` (run 42773485889, head `8cb9c41b`): 3,438 cases, 562 skipped, 0 failed, 2,876 passed. The Windows host was not reachable from the session that wrote this record, so no fresh Windows run was made on the #351 tree; both sets were compared by test id (`classname::name`).

Result: 173 skips are shared, **389 are Windows-only**, 16 are Linux-only. The 389 fall into 41 distinct reason strings, grouped into the rows below. Verdicts by test: 190 `stale` (4 rows), 84 `unclear` (5 rows), the rest `justified`.

`test/SKIP_CEILING` (193) is one number for the Linux and macOS legs and is not applied to the Windows job, which sits outside the floor.

## Windows-only skips by reason

| Reason (abridged) | Tests | Files | Verdict | Basis |
|---|---|---|---|---|
| hook launcher and its registered command are POSIX-only in v1 | 160 | 14 (`test_hook_consent` 53, `test_hook_arming` 36, `test_hooks_boundary`, `test_hook_list`, `test_hooks_lifecycle`, `test_hooks_upgrade`, ...) | stale | A native Windows launcher exists and Claude is admitted in `HOOKS_WINDOWS_VERIFIED` (`src/install/hook_registrar.rs`, `test_hooks_windows_launch.py`, `adr_hook_native_dispatcher.md`); the claim is false for Claude. Some of these still publish `sh guard.sh` payloads, which is a separate, real dependency (see next rows) |
| hook arming is POSIX-only here | 28 | `test_update_scope` | stale | Same: Windows arms hooks for admitted clients. The `hook` param still uses an `sh` handler from `make_hook` |
| payload fixtures are POSIX shell scripts (v1 launcher scope) | 61 | `test_hook_launcher` 37, `test_hook_run_runtime` 22, `test_hooks_copilot` 2 | unclear | The `#!/bin/sh` payload is a real dependency, but Claude launches through Git Bash on the Windows runner, so `sh` may be available; needs a probe, and Copilot is deliberately not admitted |
| example payloads are POSIX shell scripts (v1 launcher scope) | 9 | `test_example_hooks` | unclear | Payloads are shipped `sh` examples; a real reason unless they are run through Git Bash |
| hook launcher and payload fixtures are POSIX-only in v1 | 8 | `test_hooks_fixes_table_forward_compat` | unclear | Mixed reason: launcher half is stale, payload half is real |
| symlinked dotfiles and the hook launcher are POSIX-only | 2 | `test_hooks_fixes_symlink` | justified | Symlinks need privilege on Windows; the launcher half is stale but the symlink half holds |
| registered launcher is a POSIX shell script (v1 launcher scope) | 1 | `test_guide_agent_hooks` | stale | The registered launcher on Windows is the native `.exe` |
| needs POSIX symlinks / symlinks are a Unix layout / file or dir symlinks need elevation | 18 + 4 + 1 + 1 + 1 + 1 | `test_settings_scope`, `test_settings_status`, `test_settings_engine_root`, `test_lock_reports`, `test_symlinked_vendor_configs`, `test_mcp_artifact`, `test_mcp_cline`, `test_state_portability`, `test_desc` | justified | `os.symlink` needs a privilege the runner lacks |
| needs a Unix host (flock, symlinks, chmod) / (flock, umask, symlinks) | 16 + 9 | `test_export_marketplace`, `test_export_plugin` | justified | Mixed with mode bits and flock; part of each may run on Windows if split |
| flock(2) / fcntl.flock contention needs a Unix host | 10 + 1 | `test_lock_edges`, `test_fix_locking`, `test_update_marketplace`, `test_json_interface` | justified | Windows uses a different locking primitive; the test drives `flock` directly |
| needs a directory this process cannot write to; needs POSIX permissions/non-root user; POSIX file mode; unreadable-file mode | 8 + 1 + 1 + 1 + 1 + 2 | `test_settings_reports`, `test_settings_lifecycle`, `test_fix_add_update_wedge`, `test_fix_installer_txn`, `test_settings_file_fidelity`, `test_login`, `test_desc` | justified | Mode bits do not deny writes on Windows |
| needs POSIX symlinks and modes; needs git and POSIX symlinks | 6 + 6 | `test_settings_file_fidelity`, `test_settings_scope`, `test_settings_status` | justified | Symlink half is real; the "needs git" half is not (see candidates) |
| needs a FIFO | 2 | `test_settings_engine_root` | justified | No FIFOs on Windows |
| could not import 'pty': no module named 'termios' | 5 | `test_settings_gate` | justified | The test drives a pseudo-terminal; no pty on Windows |
| page fences are POSIX shell | 4 | `test_docs_settings_reference`, `test_guide_dependencies`, `test_guide_share_settings` | unclear | The doc fences are `sh`; whether the test only parses them or runs them decides it |
| fcntl.F_SETPIPE_SZ pipe-buffer shrinking is Linux-only | 3 | `test_broken_pipe` | justified | Linux syscall |
| shell shim / shell hook fixture / git shim / mock helper / stand-in servers / planted command are POSIX scripts | 3 + 1 + 2 + 2 + 2 + 1 + 1 | `test_publish_announce`, `test_telemetry_repository`, `test_login`, `test_stdio_drivers`, `test_index_git`, `test_harness_fixtures` | justified | The fixture itself is `sh` or Python-with-shebang; rewriting it is test work, the product behavior is covered on Linux |
| needs git | 1 | `test_settings_forged_state` | stale | Git is installed on the Windows runner; a bare "needs git" is no reason |
| POSIX symlink semantics (CWE-59) / symlinked-directory semantics | 1 + 1 | `test_path_deps`, `test_dev_install` | justified | Symlink privilege |
| 66 KB argv exceeds the Windows command line | 1 | `test_config_registry` | justified | Limit is 32,767 chars; budget covered by a Rust unit test |
| collection skipped | 2 | (module level) | unclear | No reason string; find the module-level skip |

Rows merge strings that share one cause.

## Linux-only skips (16)

| Reason | Tests | File |
|---|---|---|
| Windows launch paths | 7 | `test_hooks_windows_launch` |
| needs a Windows host (directory junctions) | 5 | `test_export_marketplace` |
| Windows rename semantics | 3 | `test_hooks_windows_replace` |
| the decline is Windows-only; POSIX hosts arm | 1 | `test_hooks_fixes_windows` |

All justified: they test Windows behavior.

## Candidates to un-skip (follow-up, not done here)

1. The 160 + 28 + 1 tests carrying "POSIX-only in v1" / "hook arming is POSIX-only here": replace the reason with a probe for the actual dependency (`sh` on PATH, via Git Bash) and run them on Windows where Claude is admitted; the biggest single win (`test_hook_consent`, `test_hook_arming`).
2. `test_settings_forged_state`: "needs git" while git is on the runner.
3. The 6 + 6 "needs git and POSIX symlinks" and the export tests: split the symlink assertion from the rest so the remainder runs.
4. `test_example_hooks`, `test_hook_launcher`, `test_hook_run_runtime`: check whether Git Bash `sh` is reachable on `windows-latest`.
5. The two module-level "collection skipped" entries: give them a reason string.
