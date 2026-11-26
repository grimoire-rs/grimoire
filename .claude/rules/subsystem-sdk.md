---
paths:
  - crates/grim-sdk/**
---

# Subsystem: grim-sdk

`crates/grim-sdk` is a small Rust client that drives the `grim` binary: find it, run it with JSON output, check a version floor, map exit codes. Status: Unstable, unpublished. Plan: `.agents/plans/plan_repository_split.md` (C-001 to C-010).

## Invariants

1. **Invokes the binary only.** It spawns `grim` (resolved from an absolute `GRIM_BIN`, else `PATH`) and reads its stdout and exit code. A needed grim fact means a CLI or JSON field in grim, never a path dependency.
2. **Links no grim crate.** `cargo tree -p grim-sdk -e normal` names none of `grimoire`, `grim-hook`, `grimoire-hook-core`. `tests/boundary.rs` pins it, so the crate can move to its own repository unchanged.
3. **Own version line.** A literal `version = "0.1.0"` in its manifest, never `version.workspace`; it moves independently of grim.
4. **Unpublished.** `publish = false`, and `dist-workspace.toml` never names `grim-sdk`.
5. **Exit-code table pinned to grim.** `ExitCode` repeats the variants and values of `src/cli/exit_code.rs`; the test `values_match_grims_exit_codes` fails when one differs. A changed grim exit code means changing both.
6. **Names no mirror concept.** `DriverError` passes codes 69, 74, 75, 80 and 81 through; any other code becomes 69.
7. **Unstable.** No stability promise until a decision says so; the SDK is not a library API of grim.

## Acceptance

`tests/against_grim.rs` is `#[ignore]` and panics without `GRIM_SDK_TEST_GRIM` (absolute path to a built `grim`). Run it with `task rust:sdk:accept`; it is part of `task verify` and the `verify-basic.yml` smoke job.
