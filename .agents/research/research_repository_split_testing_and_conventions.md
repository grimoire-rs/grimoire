# Research: Repository split, testing idea and repository conventions

## Metadata

**Date:** 2026-11-25
**Domain:** testing, ci-cd, devops
**Triggered by:** Creating two new repositories (`grim-mirror`, `grim-gui`) from "the same base" as grimoire; the owner requires that the whole testing idea travels and asked for every other convention of the same kind.
**Expires:** 2027-05-25

Scope: this repository at branch `hex/artifact-mirroring` (worktree `uno`). Agent configuration under `.claude/` and `.agents/` is covered by a separate researcher; it appears here only where a test or CI convention depends on it. Classification tokens used in the last column of every table: `as-is` (project independent, take unchanged), `pattern` (idea and structure carry over, content written anew), `grimoire-only` (no meaning outside this repository). A row for something that moves with `grim-mirror` is `grimoire-only` relative to the base and says so.

## Direct Answer

The testing idea is four layers. Rust unit tests sit inline beside the code and run under cargo-nextest. Black-box pytest acceptance tests under `test/` run the built binary as an external process against a private zot OCI registry that the pytest controller starts on a random loopback port, with a per-test `GRIM_HOME` and `HOME`. A sharded CI run merges the JUnit reports and judges the whole suite against three committed numbers (`test/SUITE_FLOOR`, `SKIP_CEILING`, `XFAIL_CEILING`). A two-tier verify (basic on every pull request, deep by manual dispatch) separates the cheap gate from the platform matrix, the older-binary interop runs and the benchmarks. Everything is driven through `task`, and CI calls the same tasks a developer runs.

The mirror suites use about one fifth of the harness. They need: top-level `test/conftest.py` (registry lifecycle, `grim_binary`, `grim_mirror_binary`, `GrimMirrorRunner`, `grim_home`, `unique_repo`), `test/tests/conftest.py` (`project_dir`, `grim_at`), `src/zot.py`, `src/registry.py`, `src/runner.py`, `src/git_fixture.py` and two functions of `src/helpers.py`. They also need a built `grim` binary (the mirror drives grim through `GRIM_BIN`), which is the one cross-repository dependency the split creates.

## A. The testing idea

### A1. Layers

| ID | Item | Evidence | Class |
|----|------|----------|-------|
| A1.1 | Rust unit tests inline beside the code (`#[cfg(test)] mod tests`), run by `cargo nextest run --workspace --release --locked`. `crates/grim-mirror/src` carries 249 `#[test]` or `#[tokio::test]` attributes (spec 34, license 34, publish 26, marketplace 24, git 23, run 18, version 17, retire 16, stage 16, bundle 14, enumerate 10, report 8, cli 6, exit_code 1, model 1). | `taskfiles/rust.taskfile.yml` task `test:unit`; `.claude/rules/quality-rust.md` "Testing Conventions" | `pattern` (nextest as the runner, inline tests; for a plugin crate the subject is its parser, planner and report types; for Tauri the shell's command handlers) |
| A1.2 | nextest profile `ci`: `fail-fast = false`, `retries = 2`, JUnit to `target/nextest/ci/junit.xml`, plus a zero-retry override for probe and index-sync tests. | `.config/nextest.toml` | `pattern` (profile and JUnit path carry over; the override filter names grimoire tests) |
| A1.3 | Rust integration test `crates/grim-mirror/tests/boundary.rs`: asserts no `grimoire` package in `cargo tree -p grim-mirror`, `publish = false`, `dist = false`, and `dist-workspace.toml` never names the crate. | `crates/grim-mirror/tests/boundary.rs` (three `#[test]`) | `grimoire-only` (exists to guard extraction; meaningless once the crate has its own repository) |
| A1.4 | Rust test fixtures `crates/grim-mirror/tests/licenses/*.txt` (SPDX text variants, `*.crlf.txt` kept byte exact by `.gitattributes`). | `.gitattributes` last line; `crates/grim-mirror/tests/licenses` | `grimoire-only` (moves with `grim-mirror`) |
| A1.5 | Pytest acceptance layer: 246 `test/tests/test_*.py` modules, 4006 lines of support code, black box on the compiled binary against a real registry. | `test/README.md`; `.claude/rules/subsystem-tests.md` "Design Rationale" | `pattern` (the whole concept; see A2) |
| A1.6 | AI-config structural tests `.claude/tests/test_ai_config.py`, `test_hooks.py`, `test_workflows.py`, `test_upstream_stale.py`, run by `task claude:tests` inside `task verify`. | `.claude/taskfile.yml` task `tests`; `taskfile.yml` `.verify:lint` | `pattern` (other researcher owns the content; the idea is "config drift fails a test") |
| A1.7 | CI structural test `.claude/tests/test_workflows.py`: `release.yml` still checked by dist, `id-token: write` only where a certificate is minted, `release-ocx.yml` starts with the main-only guard. | `.claude/tests/test_workflows.py:31,53,57` | `pattern` (for a plugin crate: pins and permissions asserted; for Tauri: the release workflow's signing guard) |
| A1.8 | Documentation checks: `task docs:check` builds the site, then runs URL contract, declaration check, theme check, landing check, `npm audit --audit-level=high`, `npm test --if-present`. Not part of `task verify`. | `taskfiles/docs.taskfile.yml` task `check` | `pattern` |
| A1.9 | Doc-example tests and documentation-versus-CLI tests inside the acceptance suite (`test_docs.py`, `test_docs_kind_lists.py`, `test_docs_mods_reference.py`, `test_docs_settings_reference.py`, `test_telemetry_config_docs.py`, `test_workflows.py` quickstart chain). | `test/tests/test_docs*.py`; `test/tests/test_workflows.py:1-25` | `grimoire-only` (they read grimoire's docs pages and CLI; the idea "docs examples run in CI" is covered by A1.8 and `doc_examples.py` under B) |
| A1.10 | Terminal cast recordings: `test/recordings/` (`cast_recorder.py`, `casts/*.yaml`), run by `task test:demo`, outside `testpaths`. | `test/taskfile.yml` task `demo` | `grimoire-only` (feeds `docs/public/` casts) |
| A1.11 | Benchmarks: `test/tests/test_index_bench.py` (marker `bench`, 20,000-entry sync budgets), `taskfiles/bench.taskfile.yml`, `task test:bench`; run by deep verify only. | `test/pyproject.toml` markers; `test/taskfile.yml` task `bench` | `grimoire-only` (index sync budgets) |
| A1.12 | Size budget gate for one binary: `rust:hook:size` builds `grim-hook` and fails above a byte budget (524288 gnu, 655360 musl); `rust:hook:audit` runs `cargo audit bin`. | `taskfiles/rust.taskfile.yml`; `.github/workflows/verify-basic.yml` jobs `hook-size`, `hook-size-windows` | `pattern` (an artifact-size budget as a CI gate: for Tauri the bundle size; for the plugin crate none is needed) |
| A1.13 | Upgrade and migration fixtures: `test/data/pre_*` frozen state trees, `test_old_grim_*.py` modules run against grim 0.15.1 and 0.16.0 by `task test:old-grim` (any skip fails, via `tools/require_all_passed.py`), `task p9:diff` (origin/main grim against branch grim). | `test/taskfile.yml` task `old-grim`; `taskfiles/p9.taskfile.yml`; `.github/workflows/verify-deep.yml` job `principle-9` | `grimoire-only` (Principle 9 compatibility contract of a released CLI; `grim-mirror` has no releases) |
| A1.14 | Index end-to-end and matrix tests (`test_index_e2e.py` marker `index_e2e`, `test_index_matrix.py`, `test/data/index_e2e/INDEXER_REF`, `test/data/conformance/`). | `test/taskfile.yml` tasks `index-e2e`, `index-matrix` | `grimoire-only` (cross-repository contract with the indexer) |
| A1.15 | Repo-level Python tests `scripts/tests/test_backfill_release.py` plus `ruff check` and `ruff format --check` on `scripts/`, task `test:scripts` in `task verify`. | `test/taskfile.yml` task `scripts` | `pattern` (lint and test the repository's own helper scripts under the same locked pytest environment) |

### A2. The acceptance harness

| ID | Item | Evidence | Class |
|----|------|----------|-------|
| A2.1 | `test/pyproject.toml`: project `grimoire-tests`, `requires-python >=3.10`, `testpaths = ["tests"]`, `pythonpath = ["."]`, markers `index_e2e` and `bench`, `tmp_path_retention_policy = "failed"` with count 1, dev group `pytest>=8.0`, `pytest-xdist>=3.5`, `pexpect>=4.9`, `rich>=14.0`, `opentelemetry-proto>=1.45.1`. | `test/pyproject.toml:1-29` | `pattern` (keep xdist, tmp retention and `testpaths`; markers and the pexpect, rich, opentelemetry dependencies are grim-only) |
| A2.2 | `test/uv.lock`: locked environment, used by `uv run`. | `test/uv.lock` | `grimoire-only` (generated; regenerate in the new repository) |
| A2.3 | `test/conftest.py` (362 lines), registry lifecycle: `pytest_configure` calls `_resolve_registry_host` (uses `GRIM_TEST_REGISTRY_HOST` if set, else `start_zot()`), then `_require_registry` aborts the session with exit 4 when no registry answers (the silent-skip-looks-green defect), keyed on `_GRIM_REGISTRY_VERIFIED`; `pytest_unconfigure` stops zot; escape hatch `GRIM_ALLOW_NO_REGISTRY`. The file must hold no top-level `src.*` import so the host is set before `src.registry` is imported. | `test/conftest.py` `pytest_configure`, `_resolve_registry_host`, `_require_registry`, `pytest_unconfigure` | `as-is` (the lifecycle is product independent; only env var names carry the `GRIM_` prefix) |
| A2.4 | `test/conftest.py` fixtures `grim_binary` (session; `GRIM_COMMAND` else `test/bin/grim(.exe)`), `grim_mirror_binary` (session; `GRIM_MIRROR_COMMAND` else `test/bin/grim-mirror`; skips on Windows), `registry`, `grim_home`, `grim`, `unique_repo` (`grim-test/<12 hex>`). | `test/conftest.py` fixtures | `pattern` (binary lookup by env var then `test/bin/`, per-test home, per-test repository name; rename for the binary under test) |
| A2.5 | `GrimMirrorRunner` in `test/conftest.py`: runs `grim-mirror` with `GRIM_BIN` set to the grim under test, and `run_via_grim` for `grim mirror ...` plugin dispatch with the mirror directory first on `PATH`. | `test/conftest.py` class `GrimMirrorRunner` | `grimoire-only` (moves with `grim-mirror`; becomes its main runner) |
| A2.6 | `test/conftest.py` `pytest_collection_modifyitems`: refuses a test id over 1000 characters. | `test/conftest.py` `_MAX_TEST_ID` | `as-is` |
| A2.7 | `test/conftest.py` autouse `_guard_host_managed_claude_settings`: skips `--global` runs on hosts whose managed Claude settings set `CLAUDE_CONFIG_DIR`. | `test/conftest.py` end of file; `test/src/runner.py` `HOST_MANAGED_CLAUDE_CONFIG_DIR` | `grimoire-only` (the mirror suites never pass `--global`) |
| A2.8 | `test/tests/conftest.py` (309 lines): `project_dir`, `bare_project_dir`, `grim_at` (factory returning a `GrimRunner` with a cwd), `two_projects`, `hostile_hook_clone`, `launcher`, `oci_proxy`, `team`, `http_index`, `kind_server`, `kind_index`, `otlp_collector`. Its module top imports `src.index_fixture`, `src.otlp_fixture` and `src.runner`. | `test/tests/conftest.py:1-310` | `pattern` (only `project_dir` and `grim_at` matter to the mirror; the rest is grim-only) |
| A2.9 | `src/__init__.py` (23 lines): re-exports `GrimRunner`, `current_platform` and four assertions; deliberately does not import `src.helpers` (it would import `src.registry` before the host is set). | `test/src/__init__.py` | `pattern` |
| A2.10 | `src/assertions.py` (37): `assert_path_exists`, `assert_dir_exists`, `assert_symlink_exists` (accepts Windows junctions), `assert_not_exists`. | `test/src/assertions.py` | `as-is` |
| A2.11 | `src/consent_driver.py` (101): pty-driven workspace consent question and an older consent record. | `test/src/consent_driver.py` | `grimoire-only` (hook and settings consent) |
| A2.12 | `src/git_fixture.py` (199): deterministic local git upstreams (`GitRepo`, pinned identity and dates, `GIT_CONFIG_GLOBAL=/dev/null`, no signing, `file://` urls) and builders `skill_md`, `rule_md`, `agent_md`, `monorepo`, `marketplace`. Line 18 reads `crates/grim-mirror/licenses/MIT.txt` by a path relative to the repository root. | `test/src/git_fixture.py:18,43-199` | `pattern` (`GitRepo` is generic; the builders and the MIT path are mirror specific; for the plugin crate they stay, for Tauri they are unused) |
| A2.13 | `src/helpers.py` (816): `write_config` (writes `grimoire.toml`), `make_artifact`, `make_bundle` and many grim-specific builders; imports `src.registry`. | `test/src/helpers.py:17,64,644` | `grimoire-only` (the mirror suites import only `write_config` and `make_artifact`; a new repository needs a slimmed copy of those two) |
| A2.14 | `src/hook_launcher.py` (104): the real hook launcher installed by `grim install`. | `test/src/hook_launcher.py` | `grimoire-only` |
| A2.15 | `src/index_fixture.py` (446): static index dist server in several published shapes. | `test/src/index_fixture.py` | `grimoire-only` |
| A2.16 | `src/oci_proxy.py` (155): counting, recording or hanging HTTP proxy in front of the registry. | `test/src/oci_proxy.py` | `grimoire-only` (generic idea, but only grim's transport tests use it) |
| A2.17 | `src/old_grim.py` (87): locates the 0.15.1 and 0.16.0 binaries (`GRIM_OLD_BIN`, `GRIM_OLD_BIN_0_16_0`). | `test/src/old_grim.py` | `grimoire-only` |
| A2.18 | `src/otlp_fixture.py` (256): capture OTLP/HTTP logs collector; needs `opentelemetry-proto`. | `test/src/otlp_fixture.py:36` | `grimoire-only` |
| A2.19 | `src/registry.py` (359): stdlib-only OCI client (`push_artifact`, `push_manifest`, `fetch_manifest`, `fetch_blob`, `list_tags`, `tag_digest`, `retag`), reads `REGISTRY_HOST` from `GRIM_TEST_REGISTRY_HOST` at import; hard-codes `application/vnd.grimoire.artifact.layer.v1.tar`, `com.grimoire.kind`, `com.grimoire.uncompressed.digest`. | `test/src/registry.py:43-60` | `pattern` (the client is generic; media types and annotation keys are grim's) |
| A2.20 | `src/runner.py` (167): `GrimRunner` with per-instance env (`GRIM_HOME`, minimal `PATH`, isolated `HOME`, `USERPROFILE`, `APPDATA`, `XDG_CONFIG_HOME`, Windows spawn variables, `GRIM_INSECURE_REGISTRIES` when the registry host is not a built-in loopback form), `run`, `json`, `plain`, UTF-8 decoding, `stdin=DEVNULL`. | `test/src/runner.py:43-167` | `pattern` (isolated-environment subprocess runner; names and variables are grim's) |
| A2.21 | `src/scenarios.py` (382): multi-machine and multi-workspace helpers. | `test/src/scenarios.py` | `grimoire-only` |
| A2.22 | `src/stdio_drivers.py` (257): stdio, MCP and hook-run drivers. | `test/src/stdio_drivers.py` | `grimoire-only` |
| A2.23 | `src/zot.py` (139): starts zot on a free port, retries 5 times, runs it under a watchdog that kills it on stdin EOF so no registry outlives a SIGKILLed session; error text names `ocx.toml`; temp prefix `grim-zot-`. | `test/src/zot.py:30-100` | `as-is` (two cosmetic strings) |
| A2.24 | Test data under `test/data/`: `conformance/`, `convert/`, `golden/`, `index_e2e/`, `index_flat_unchanged/`, `publish_mirror/` (read only by `test_publish_mirror_table.py`), `pre_*` state trees, `claude-*` fixtures. None is read by the ten mirror suites. | `ls test/data`; `test/tests/test_publish_mirror_table.py:15` | `grimoire-only` |
| A2.25 | `test/bin/` (gitignored): where `task test:build` copies `grim` and `grim-mirror`; CI downloads artifacts into it and renames to the plain name. | `.gitignore` `test/bin/`; `test/taskfile.yml:build`; `.github/workflows/acceptance.yml:77-103` | `pattern` |
| A2.26 | Floor and ceilings: `test/SUITE_FLOOR` (4494), `test/SKIP_CEILING` (296), `test/XFAIL_CEILING` (0). Enforced by `tools/accept_floor.py` in the CI merge job (`task test:floor`); warn-only locally (`test:floor:warn`). Refuses a failed case, a missing shard, a duplicate `classname.name`, a count under the floor, skips or xfails over the ceilings, and a "parked test" marker in `src`, `test` or `crates`. | `test/SUITE_FLOOR`, `SKIP_CEILING`, `XFAIL_CEILING`; `test/tools/accept_floor.py:1-50`; `.github/workflows/acceptance.yml` job `merge` | `pattern` (mechanism as-is; the three numbers are written anew; the park marker string `lock-pin-park` is grim-only) |
| A2.27 | `tools/accept_floor.py` (167), `tools/accept_shard.py` (55): shard `I` of `N` takes `modules[I::N]` of the sorted `tests/test_*.py` stems. | `test/tools/accept_shard.py:1-55` | `as-is` (apart from the park marker constant, see A2.26) |
| A2.28 | `tools/require_all_passed.py` (78): fails a report on any failed, skipped or xfailed case or a module with no case. | `test/tools/require_all_passed.py:1-12` | `as-is` |
| A2.29 | `tools/p9_diff.py` (1221) and `tools/p9_fixtures/`. | `taskfiles/p9.taskfile.yml` | `grimoire-only` |
| A2.30 | Meta tests of the harness: `test_accept_floor.py` (360), `test_accept_shard.py` (106), `test_text_io_hygiene.py` (AST check that every `read_text`, `write_text` and text-mode `open` under `test/` names encoding and newline, except `.venv/` and `data/`). | `test/tests/test_text_io_hygiene.py:1-25` | `as-is` |
| A2.31 | `test/taskfile.yml`: vars `GRIM_BINARY`, `GRIM_MIRROR_BINARY`, `SKIP_BUILD` (defaults to `CI`); tasks `build` (internal), `default`, `quick`, `parallel`, `shard`, `floor`, `floor:warn`, `scripts`, `index-e2e`, `index-matrix`, `old-grim`, `bench`, `demo`. `default` runs `ocx exec -- uv run pytest -m "not index_e2e and not bench"` with `GRIM_COMMAND` and `GRIM_MIRROR_COMMAND` in `env`. | `test/taskfile.yml` | `pattern` (build, default, quick, parallel, shard, floor survive; the other tasks are grim-only) |
| A2.32 | `test/README.md` (86 lines): directory structure, fixtures, isolation, configuration. | `test/README.md` | `pattern` |
| A2.33 | `test/manual/` (docker-compose registries, manual project rigs). | `ls test/manual` | `grimoire-only` |
| A2.34 | Parallelism: `-n auto` (xdist) in `quick`, `parallel`, `shard`, `old-grim`; the controller starts one zot and workers inherit `GRIM_TEST_REGISTRY_HOST` at fork. | `test/taskfile.yml`; `test/conftest.py` `_resolve_registry_host` docstring | `pattern` |
| A2.35 | Platform handling: `pytest.mark.skipif(sys.platform == "win32", ...)` per test (symlinks, exec bit, backslash paths, shell stand-in), `grim_mirror_binary` skips on Windows ("no mirror binary is built for that target"), Windows runs only in deep verify. | `test/tests/test_mirror_staging.py:51,62,89`; `test/tests/test_mirror_cli.py:69`; `test/conftest.py` `grim_mirror_binary` | `pattern` |
| A2.36 | How output is asserted: no snapshot files; each test parses `stdout` with `json.loads` (`--format json`) and asserts fields, asserts `returncode == <n>` exactly (64, 65, 69, 78, 79, 81 in `test_mirror_cli.py`) and `in r.stderr` substrings; `check=False` when a nonzero exit is the subject. Golden JSON only for schemas (`test/data/golden/`). | `test/tests/test_mirror_cli.py:45-136` | `pattern` |
| A2.37 | Naming: modules `test_<area>.py` (mirror suites `test_mirror_<area>.py`); functions `test_<subject>_<behaviour as a sentence>`; one `def _helper` per module for the file's fixture builders; test ids capped at 1000 characters. | `test/tests/test_mirror_*.py`; `test/conftest.py` `_MAX_TEST_ID` | `as-is` |
| A2.38 | Every Python file carries the two-line SPDX header (`# SPDX-License-Identifier: Apache-2.0`, `# Copyright 2026 The Grimoire Authors`) and `from __future__ import annotations`; 9 of 246 test modules lack the header. | `test/tests/test_mirror_cli.py:1-4`; `grep -L SPDX` count 9 | `pattern` (hawkeye `includes` only list Rust globs, so Python headers are convention, not enforced) |

### A3. The gate

| ID | Item | Evidence | Class |
|----|------|----------|-------|
| A3.1 | `task verify`: phase 1 `.verify:lint` runs in parallel `rust:format:check`, `rust:clippy:check`, `shell:verify`, `claude:verify`, `test:scripts`; phase 2 `.verify:build-test` runs in sequence `rust:build`, `catalog:verify`, `rust:test:unit`, `test:parallel`; then `.verify:mark` writes `date +%s` to `.claude/hooks/.state/commit-verified`. Docs gate is separate. | `taskfile.yml` `verify`, `.verify:lint`, `.verify:build-test`, `.verify:mark` | `pattern` |
| A3.2 | `task --force verify` bypasses task caching; the summary says "Use `task --force verify` to bypass caching". | `taskfile.yml` `verify.summary` | `as-is` |
| A3.3 | `task rust:verify`: `format:check`, `clippy:check`, `license:deps`, `build`, `test:unit` in order, fail fast. `clippy:check` is `cargo clippy --workspace --locked --all-targets -- -D warnings`. | `taskfiles/rust.taskfile.yml` `verify`, `clippy:check` | `pattern` |
| A3.4 | Caching rules: `sources:` globs on rust tasks (anchored `*rust-sources` YAML alias: `src`, `crates`, `build`, `Cargo.toml`, `Cargo.lock`, `rustfmt.toml`, excluding `target/`); `build` uses `status: test -f target/release/grim` with `method: timestamp`; `test:parallel` lists `tests/**/*.py`, `src/**/*.py`, conftests, `pyproject.toml` and the release binary as sources; `SKIP_BUILD` defaults to `CI`. | `taskfiles/rust.taskfile.yml:6-17,60-66`; `test/taskfile.yml` `parallel.sources` | `pattern` |
| A3.5 | Commit-verified stamp: `.verify:mark` writes the epoch seconds; the Claude hook `pre_commit_verification.py` denies a `git commit` unless the stamp is under 5 minutes old and covers only this repository's worktrees. | `taskfile.yml` `.verify:mark`; `.claude/hooks/pre_commit_verification.py:1-8` | `pattern` (the stamp is task side; the hook is agent configuration owned by the other researcher) |
| A3.6 | `task docs:check`: `build` first, then `check_urls.py`, `doc_declaration.py`, `theme_check.sh`, `landing_check.py`, `npm audit --audit-level=high`, `npm test --if-present`; alias `docs:verify`; needs Node 24. | `taskfiles/docs.taskfile.yml` `check` | `pattern` |
| A3.7 | Lint never blocks test results: `continue-on-error: true` on fmt, clippy and supply-chain steps with a final gate step. | `.github/workflows/verify-basic.yml:114-120,155-161`; `.claude/rules/subsystem-ci.md` principle 3 | `as-is` |

### A4. Binding test rules written down

| ID | Rule | Source | Class |
|----|------|--------|-------|
| A4.1 | Acceptance tests are black box: build the binary, run it as an external process, assert only on stdout, stderr and exit codes. | `test/README.md:3-5`; `.claude/rules/subsystem-tests.md` | `as-is` |
| A4.2 | Per-test isolation: own `GRIM_HOME`, own `HOME` (sibling of it), stripped environment; `unique_repo` per test on the shared registry, no cleanup. | `subsystem-tests.md` "Test Isolation" | `pattern` |
| A4.3 | The registry is required; an unreachable registry aborts the session; `GRIM_ALLOW_NO_REGISTRY` is "NOT a valid quality gate". | `test/conftest.py` `_require_registry` | `as-is` |
| A4.4 | Text I/O under `test/` names its codec and newline (enforced by a test). | `subsystem-tests.md`; `test/tests/test_text_io_hygiene.py` | `as-is` |
| A4.5 | Use `assert_symlink_exists`, never `path.is_symlink()` (Windows junctions). | `subsystem-tests.md` "Assertion Helpers" | `pattern` (only if the new repository tests on Windows) |
| A4.6 | A pinned digest test must keep the manifest tagged (`retag(..., "keep", ...)`): zot drops an untagged manifest. | `subsystem-tests.md` "Design Rationale" | `pattern` |
| A4.7 | Floor and ceilings enforced in PR CI; local runs only warn. | `subsystem-tests.md` "Structure" | `pattern` |
| A4.8 | Bug fixes: Reproduce, root cause, failing regression test first, fix, verify, document; "every bug fix gets a failing test". | `.claude/rules/workflow-bugfix.md:17-30`; `AGENTS.md` Principle 2 | `as-is` |
| A4.9 | A new refusal reason, report field or `mirror.toml` key needs a unit test, an acceptance case and a docs page update in the same change. | `.claude/rules/subsystem-mirror.md` "Testing" | `grimoire-only` (moves with `grim-mirror`) |
| A4.10 | Rust: workspace lints `unsafe_code = "forbid"`, clippy `unwrap_used` and `expect_used` at `warn` (tests exempt), `#[cfg(test)] impl` block before `mod tests`. | `Cargo.toml:173-186`; `quality-rust.md` "Testing Conventions" | `as-is` |
| A4.11 | Python: no bare `except`, no `assert` for validation, no mutable defaults, no wildcard imports, typed public functions, modern generics, `from e` chaining. | `.claude/rules/quality-python.md:20-34` | `as-is` |
| A4.12 | Always run `task verify` after implementation; `cargo fmt` before commit; subsystem gate during review loops, full gate before commit. | `AGENTS.md` "Build & Development Commands" | `as-is` |
| A4.13 | Verification honesty: no "should work", "probably"; a "verified" claim cites evidence. | `.claude/rules/quality-core.md` "Verification Honesty" | `as-is` |
| A4.14 | Test-subject documentation: `subsystem-tests.md` itself, including its Structure and Key Fixtures tables, is path scoped to `test/**` and must be rewritten for each repository. | `.claude/rules/subsystem-tests.md:1-4` | `pattern` |

### A5. What the ten mirror suites and `crates/grim-mirror/tests/` use

Suites: `test_mirror_bundles`, `_cli`, `_dialect`, `_kinds`, `_license`, `_marketplace`, `_monorepo`, `_retire`, `_staging`, `_versions` (1754 lines, 51 test functions counted from `def test_`; 133 uses of the `grim_mirror` fixture in signatures). `test_publish_mirror_table.py` and `test/data/publish_mirror/` are grim-side, not mirror tests, and stay in grimoire.

Used:

| Part | Used by | Detail |
|------|---------|--------|
| `grim_mirror` fixture and `GrimMirrorRunner` | all ten | `run(...)`, `.env` overridden for `GRIM_OFFLINE=1` and `GRIM_BIN=<fake>` in `test_mirror_cli.py:53,79`; `run_via_grim` in `test_mirror_cli.py:107,150` |
| `registry`, `unique_repo`, `tmp_path` | all ten | the mirror registry is `f"{registry}/{unique_repo}"` |
| `grim_at` | cli, bundles, dialect, kinds, license, marketplace, monorepo, retire, staging, versions (27 uses) | drives `describe`, `fetch`, `add`, `release`, `status`, `convert` of the real `grim` |
| `project_dir` | bundles, marketplace, retire, staging, dialect (12 uses) | `.claude/` marker workspace |
| `grim_binary`, `grim_mirror_binary`, `grim_home` | indirectly through the two fixtures above | env vars `GRIM_COMMAND`, `GRIM_MIRROR_COMMAND`, `GRIM_BIN`, `GRIM_OFFLINE`, `GRIM_TEST_REGISTRY_HOST`, `GRIM_INSECURE_REGISTRIES` (via `GrimRunner`), `GRIM_HOME` |
| `pytest_configure` registry lifecycle | whole session | zot from `src/zot.py`, `ocx exec`/`setup-ocx` supplying `zot` and `uv` |
| `src/git_fixture.py` | all ten | `GitRepo`, `BASE_DATE`, `skill_md`, `agent_md`, `monorepo`, `marketplace`; `MIT_TEXT` read from `crates/grim-mirror/licenses/MIT.txt` (also read directly in `test_mirror_bundles.py:19`) |
| `src/registry.py` | bundles, marketplace, monorepo, staging, versions | `fetch_manifest`, `fetch_blob`, `list_tags` |
| `src/helpers.py` | bundles, dialect, marketplace, retire, staging | `write_config` (10 call sites), `make_artifact` (dialect) |
| `src/runner.py` | indirectly | `GrimRunner` builds the isolated environment that `GrimMirrorRunner` copies |
| pytest marks | cli, staging | `skipif(sys.platform == "win32")` |
| CI plumbing | all | `test/bin/grim-mirror` from artifact `grim-mirror-binary` (`verify-basic.yml:130-138,171-172`), per-target in deep verify (`verify-deep.yml:32-40,200-201`) |
| Rust side | `crates/grim-mirror/tests/boundary.rs`, `tests/licenses/`, `licenses/`, 249 inline unit tests | `boundary.rs` reads `../../dist-workspace.toml` and runs `cargo tree` at the repository root |

Not used by the mirror suites (they stay with grimoire): `bare_project_dir`, `two_projects`, `hostile_hook_clone`, `launcher`, `oci_proxy`, `team`, `http_index`, `kind_index`, `otlp_collector`; `src/assertions.py`, `consent_driver.py`, `hook_launcher.py`, `index_fixture.py`, `oci_proxy.py`, `old_grim.py`, `otlp_fixture.py`, `scenarios.py`, `stdio_drivers.py`; all of `test/data/` except what `test_publish_mirror_table.py` reads; markers `index_e2e` and `bench`; the old-grim, p9, index-e2e, matrix, bench and demo tasks. They are still imported at collection time: `test/tests/conftest.py` imports `src.index_fixture` and `src.otlp_fixture` (the latter needs `opentelemetry-proto`) at module top, so a copy of `tests/conftest.py` must be trimmed or the dependency stays.

### A6. Harness parts that hard-code grim specifics

| Location | Hard-coded | Needed by mirror |
|----------|-----------|------------------|
| `test/conftest.py` | `grim_binary`, `GRIM_COMMAND`, `test/bin/grim`, `grim_mirror_binary`, `GRIM_MIRROR_COMMAND`, `GRIM_BIN`, `unique_repo` prefix `grim-test/`, `CLAUDE_CONFIG_DIR` managed-settings guard, env names `GRIM_TEST_REGISTRY_HOST`, `GRIM_ALLOW_NO_REGISTRY` | yes, mostly |
| `test/src/runner.py` | `GRIM_HOME`, `GRIM_INSECURE_REGISTRIES`, class name `GrimRunner`, subcommand-free `--format`/`--log-level` flags | yes |
| `test/src/registry.py` | layer media type `application/vnd.grimoire.artifact.layer.v1.tar` and gzip, `com.grimoire.kind`, `com.grimoire.uncompressed.digest`, artifact type derivation | partly (`fetch_*`, `list_tags` only) |
| `test/src/helpers.py` | `grimoire.toml` writer, kinds, `.claude` client marker, state file names | two functions |
| `test/tests/conftest.py` | `.claude` marker dir as the "detected Claude project", `grimoire.toml` discovery by walking up from cwd | `project_dir`, `grim_at` |
| `test/src/git_fixture.py` | path `crates/grim-mirror/licenses/MIT.txt`, skill, rule, agent markdown shapes | yes |
| `test/src/zot.py` | `grim-zot-` temp prefix, `ocx.toml` in an error message | yes |
| `test/taskfile.yml` | `GRIM_BINARY`, `GRIM_MIRROR_BINARY`, `ocx exec`, `grim` 0.14.3 group `g0`, `GRIM_INDEXER_DIR`, `GRIM_OLD_BIN*` | the build/default/shard/floor tasks |
| `test/tools/accept_floor.py` | park marker `lock-pin-park` (split in the source so the scan misses itself) | optional |
| State paths | `$GRIM_HOME/state/global.json`, `<workspace>/.grimoire/state.json`, `$GRIM_HOME/hooks/`, `consent/`, `settings/`, `mods/` appear only in grim-only suites and `src/consent_driver.py`, `hook_launcher.py` | no |

## B. Other repository conventions

### B1. CI

| ID | Item | Evidence | Class |
|----|------|----------|-------|
| B1.1 | `verify-basic.yml` "Basic Verification": trigger `push` to `main` and `pull_request` to `main`; permissions `contents: read`, `checks: write`, `pull-requests: write`; concurrency group `workflow-ref` with `cancel-in-progress` except on `main`. Jobs: `conventional-commits` (cocogitto over the PR range), `dco` (`task git:dco`), `supply-chain` (cargo-deny licences and advisories), `smoke` (fmt, clippy, build, `catalog:verify`, build `grim-mirror`, nextest `--profile ci`, upload both binaries, publish unit results, lint gate step), `acceptance-tests` (reusable workflow, 4 shards), `hook-size` (gnu and musl), `hook-size-windows` (cargo-xwin container). | `.github/workflows/verify-basic.yml:1-30,31-205` | `pattern` (cocogitto, DCO, supply chain, smoke, acceptance stay; plugin crate: build the plugin and the pinned grim, run pytest; Tauri: front-end lint, typecheck, unit tests, `cargo tauri build` smoke) |
| B1.2 | `verify-deep.yml` "Verify Deep": `workflow_dispatch` only; matrix Linux gnu and macOS aarch64 build and unit test, Windows x64 and ARM64 cross-compile in `messense/cargo-xwin`, `hook-size-macos`, `hooks-windows`, acceptance on Linux and macOS, `platform-check-windows`, `principle-9`, `index-e2e` (with matrix and bench), `mod-load-probe`. Deep is never a pre-merge gate. | `.github/workflows/verify-deep.yml:1-35,56-68,187-206` | `pattern` (the tier split and the platform matrix; plugin crate: Linux and macOS acceptance, Windows skipped as today; Tauri: three-OS bundle build) |
| B1.3 | `acceptance.yml`: reusable (`workflow_call`) sharded suite: `plan` (shard list), `shards` (matrix, downloads `test/bin`, `setup-uv`, `setup-task`, `setup-ocx` version 0.6.8, `task test:shard`, uploads `accept-junit-<os>-shard-<i>` with `overwrite: true`, `retention-days: 3`), `merge` (`task test:floor`, merged `junit.xml`, `if: !cancelled()`), `publish` (the only job with `checks: write` and `pull-requests: write`, checks out nothing). Artifact names carry `runner-os`. No `concurrency:` block on purpose. | `.github/workflows/acceptance.yml:1-30,60-113,134-185,187-217` | `pattern` (artifact and input names change; the shard, merge, publish-without-checkout structure carries over) |
| B1.4 | `release.yml`: cargo-dist generated (`cargo-dist-version 0.33.0`), `workflow_dispatch` from main, `plan`, `build-local-artifacts`, `build-global-artifacts`, `host`, `announce`, then custom jobs `release-ocx` and `publish-catalog`; excluded from dependabot. | `.github/workflows/release.yml:16-60,330-347`; `dist-workspace.toml` | `pattern` (plugin crate: `dist = false`, no release; Tauri: a release workflow built on `tauri-action`, signing and updater keys) |
| B1.5 | `release-ocx.yml` plus composite action `.github/actions/release-ocx/`: signs and pushes the grimoire OCX package with keyless Sigstore; refuses any ref but `refs/heads/main`. | `.github/workflows/release-ocx.yml:23-82` | `grimoire-only` |
| B1.6 | `publish-catalog.yml`: publishes the first-party catalog with the released grim. | `.github/workflows/publish-catalog.yml:35-70` | `grimoire-only` |
| B1.7 | `backfill.yml`: manual backfill of release assets into OCX; main-only guard. | `.github/workflows/backfill.yml:23-60` | `grimoire-only` |
| B1.8 | `claude.yml`: runs `anthropics/claude-code-action` on issue comments, review comments and issues. | `.github/workflows/claude.yml:1-40` | `pattern` (an agent-on-mention workflow; belongs with the agent configuration researcher) |
| B1.9 | `docs.yml`: on push (path filtered) and dispatch, `task docs:build`, `task docs:check`, GitHub Pages deploy with a retry deploy step on failure. | `.github/workflows/docs.yml:9-115` | `pattern` (plugin crate: a small docs site or README only; Tauri: product site or none) |
| B1.10 | Composite action `.github/actions/build-rust/action.yml`: toolchain, rust-cache (`save-if` main), `cargo build --release --target --locked`, rename to `grim-<target>[.exe]`, upload artifact `retention-days: 7`. | `.github/actions/build-rust/action.yml` | `pattern` (binary name and artifact name) |
| B1.11 | Action pinning: every `uses:` is a full commit SHA with a `# vX.Y.Z` comment; `release.yml` is the exception (dist output, tags like `@v6`). | `.github/workflows/*.yml`; `.claude/rules/subsystem-ci.md` principle 5 | `as-is` |
| B1.12 | Permissions and injection rules: workflow-level `permissions` minimal and elevated per job; `permissions: {}` on `acceptance.yml` and `backfill.yml`; `persist-credentials: false` where the job runs branch code; SHAs and inputs reach shell through `env:`, never inline `${{ }}`; `id-token: write` only in two workflows. | `acceptance.yml:36,66-68`; `verify-basic.yml:43-47`; `subsystem-ci.md` principle 5 | `as-is` |
| B1.13 | Concurrency: `${{ github.workflow }}-${{ github.ref }}`, `cancel-in-progress` false on `main`; reusable workflows carry none (a duplicate group would deadlock). | `verify-basic.yml:12-17`; `acceptance.yml:9-11` | `as-is` |
| B1.14 | Platform matrix: basic is Linux x64 only; deep adds macOS aarch64 and Windows x64 and ARM64 (cross-compiled with cargo-xwin, tested on `windows-latest`); release targets 8 triples (aarch64 and x86_64 for macOS, Linux gnu, Linux musl, Windows). `grim-mirror` has no Windows build. | `verify-deep.yml`; `dist-workspace.toml` `targets` | `pattern` |
| B1.15 | Artifact passing: build once in `smoke`, upload with `compression-level: 0` and `retention-days: 3` (a shard rerun days later still finds it), downstream jobs download; binary renamed to the plain name in `test/bin`. | `verify-basic.yml:132-148`; `acceptance.yml:77-103` | `as-is` |
| B1.16 | `.github/dependabot.yml`: weekly; `github-actions` for `/` and `/.github/actions/build-rust` grouped, `release.yml` excluded, commit prefix `ci(deps)`; `cargo` grouped `rust-deps`, prefix `chore(deps)`; `npm` for `/docs` grouped. No renovate. | `.github/dependabot.yml` | `pattern` (Tauri adds an `npm` entry for the front end root; each ecosystem needs its own directory entry, as the `build-rust` comment warns) |
| B1.17 | `.github/CODEOWNERS`: `* @michael-herwig`. | `.github/CODEOWNERS` | `as-is` |
| B1.18 | Issue templates `bug_report.yml`, `feature_request.yml`, `config.yml` (`blank_issues_enabled: true`, a Discussions contact link). No pull request template exists. | `.github/ISSUE_TEMPLATE/` | `pattern` (fields name the product, version and registry) |
| B1.19 | CI calls tasks, not raw commands: `task rust:format:check`, `task rust:clippy:check`, `task rust:build`, `task test:shard`, `task docs:check`; raw commands only for CI glue. | `subsystem-ci.md` principle 1; `verify-basic.yml` | `as-is` |

### B2. Task runner

| ID | Item | Evidence | Class |
|----|------|----------|-------|
| B2.1 | `taskfile.yml`: Taskfile v3, `includes` (`bench`, `git`, `rust`, `shell`, `coverage`, `release`, `schema`, `docs`, `upstream`, `p9` as `taskfiles/*.taskfile.yml`; `claude`, `test`, `catalog` as subsystem taskfiles with `dir:`), default task = fmt check + clippy + `cargo check`, `.ensure-cargo-tool` helper, `checkpoint`, `push`, `verify`. | `taskfile.yml:1-130` | `pattern` |
| B2.2 | `taskfiles/rust.taskfile.yml`: `verify`, `format:check`, `format:apply`, `clippy:check`, `clippy:fix`, `build`, `test:unit`, `license:check`, `license:format`, `license:deps`, `audit`, `hook:size`, `hook:audit`; tools installed on demand by `.ensure-cargo-tool` (`status: <tool> --version`). | `taskfiles/rust.taskfile.yml` | `pattern` (the first twelve names carry over; Tauri adds front-end tasks `web:lint`, `web:test`, `web:build`) |
| B2.3 | `taskfiles/shell.taskfile.yml`: shellcheck and `shfmt -d -i 4 -ci` on `git ls-files '*.sh' '*.bash'`, skipped when the tool is absent. | `taskfiles/shell.taskfile.yml:6-40` | `as-is` |
| B2.4 | `taskfiles/git.taskfile.yml`: `merge` (fast-forward onto main, fails on dirty tree), `dco` (every non-merge commit in the range has a `Signed-off-by` trailer whose email matches the author; `[bot]` exempt). | `taskfiles/git.taskfile.yml` | `as-is` |
| B2.5 | `taskfiles/docs.taskfile.yml`: `build`, `serve`, `check` (alias `verify`), `GRIM_VERSION` read from `Cargo.toml`. | `taskfiles/docs.taskfile.yml:1-100` | `pattern` |
| B2.6 | `taskfiles/release.taskfile.yml`: `install:cargo-edit`, `changelog`, `changelog:preview`, `prepare`. | `taskfiles/release.taskfile.yml` | `pattern` |
| B2.7 | `taskfiles/coverage.taskfile.yml`: `cargo llvm-cov` to `lcov.info` and HTML. | `taskfiles/coverage.taskfile.yml` | `as-is` |
| B2.8 | `taskfiles/schema.taskfile.yml`, `bench.taskfile.yml`, `p9.taskfile.yml`, `upstream.taskfile.yml`, `catalog/taskfile.yml`. | `taskfiles/`; `catalog/taskfile.yml` | `grimoire-only` |
| B2.9 | `.claude/taskfile.yml` (`claude:verify`, `tests`, `lint:links`, `statusline:install`). | `.claude/taskfile.yml` | `pattern` (other researcher) |
| B2.10 | `.taskrc.yml`: `interactive: true`, `failfast: true`. | `.taskrc.yml` | `as-is` |
| B2.11 | Conventions: dot-prefix for internal tasks (`.ensure-cargo-tool`, `.verify:lint`), `internal: true`, task-level `dir:` (a per-command `dir:` is ignored), `desc` and `summary` on public tasks, `sources`/`status`/`generates` for caching, shared source lists as YAML anchors under `x-shared`, "always check `task --list` before inventing ad-hoc commands". | `taskfile.yml` comments; `.claude/rules/subsystem-taskfiles.md`; `AGENTS.md` | `as-is` |

### B3. Rust project conventions

| ID | Item | Evidence | Class |
|----|------|----------|-------|
| B3.1 | Cargo workspace: `members = [hook-core, grim-hook, grim-mirror]`, `exclude = ["target"]`, `resolver = "3"`, single `[workspace.package]` site for `version`, `edition = "2024"`, `license = "Apache-2.0"` inherited by members; `cargo set-version` bumps all. | `Cargo.toml:1-16` | `pattern` (plugin crate: one crate or a one-member workspace; Tauri: workspace with `src-tauri` as member) |
| B3.2 | Workspace lints: `unsafe_code = "forbid"`, clippy `unwrap_used` and `expect_used` `warn`; members opt in with `[lints] workspace = true`. | `Cargo.toml:173-186`; `crates/grim-mirror/Cargo.toml` `[lints]` | `as-is` (Tauri's generated shell may need an `unsafe` exception for its own glue; not verified) |
| B3.3 | `rustfmt.toml`: `max_width = 120`. | `rustfmt.toml` | `as-is` |
| B3.4 | `clippy.toml`: `disallowed-types` for `ratatui::style::Color` and `Stylize`. | `clippy.toml` | `grimoire-only` (TUI colour rule) |
| B3.5 | `rust-toolchain.toml`: channel `1.95.0`, profile default, components `rustfmt`, `clippy`. | `rust-toolchain.toml` | `as-is` |
| B3.6 | `deny.toml`: licence allow list (12 SPDX ids), confidence 0.8, `yanked = "warn"`, `unmaintained = "workspace"`, `multiple-versions = "warn"`, `unknown-registry = "deny"`, `unknown-git = "allow"`; run as `task rust:license:deps` and `task rust:audit` (network, hard gate). | `deny.toml:1-45`; `rust.taskfile.yml` | `pattern` (Tauri's dependency tree needs its own allow list; `unknown-git = "allow"` exists for the submodule patch only) |
| B3.7 | `.cargo/config.toml`: absent. `rust-version` (MSRV): absent in every manifest; the toolchain pin is the only floor. | `ls .cargo` fails; `grep rust-version` empty | `grimoire-only` (negative finding; see `negative:`) |
| B3.8 | Cargo flags: `--locked` on every build, clippy and test; `Cargo.lock` committed. | `rust.taskfile.yml`; `test/taskfile.yml` | `as-is` |
| B3.9 | Error and exit-code rules: `quality-rust-errors.md` (typed errors) and `quality-rust-exit_codes.md` (sysexits style codes; the mirror uses 64, 65, 69, 78, 79, 81). | `.claude/rules/quality-rust-errors.md`, `quality-rust-exit_codes.md`; `crates/grim-mirror/src/exit_code.rs`; `test_mirror_cli.py` | `pattern` (plugin crate: its own exit-code table with acceptance tests per code; Tauri: error type per command, no exit codes) |
| B3.10 | `[profile.dist]`, `[profile.hook]`, `[profile.hook-dev]`; `[patch.crates-io]` pointing at `external/` submodules (`docker_credential`, `rust-oci-client`) with `.gitmodules`. | `Cargo.toml:164-202`; `.gitmodules` | `grimoire-only` |
| B3.11 | Tech strategy: Rust 2024, Tokio, Mold linker (dev); `grim-hook` deviation (sync `std`, dependency allowlist test). | `.claude/rules/product-tech-strategy.md` | `pattern` |
| B3.12 | `build.rs` and `build/` generators; `assets/`. | `build.rs`; `build/`; `assets/` | `grimoire-only` |

### B4. Commit and branch conventions

| ID | Item | Evidence | Class |
|----|------|----------|-------|
| B4.1 | Conventional Commits, validated in PRs by cocogitto over `base..head`; `cog.toml` adds the `release` commit type. | `verify-basic.yml:20-32`; `cog.toml:1-5` | `as-is` (drop or keep the `release` type) |
| B4.2 | DCO: `git commit -s`; `task git:dco` in CI and locally; CONTRIBUTING explains inbound equals outbound licensing. | `taskfiles/git.taskfile.yml`; `CONTRIBUTING.md:55-93` | `as-is` |
| B4.3 | Signed commits (GPG or SSH): not enforced anywhere in the repository. | no match in `.github`, taskfiles or `.claude/hooks` | `grimoire-only` (negative finding) |
| B4.4 | `cliff.toml`: git-cliff, Keep a Changelog header, groups by commit type, "customized for Grimoire". | `cliff.toml:1-30` | `pattern` |
| B4.5 | Branch model: never commit to `main`, branch per task, fast-forward only via `task git:merge`, `/finalize` rewrites checkpoint commits, never push without the human (hook `pre_push_main_blocker.py`). | `CONTRIBUTING.md:71-75`; `AGENTS.md` Principle 6; `.claude/hooks/pre_push_main_blocker.py` | `pattern` |
| B4.6 | Worktree layout: primary checkout on `main`, siblings `../<repo>-wt-<topic>` or `.agents/worktrees/<topic>`. | `AGENTS.md` "Workflow" | `pattern` (other researcher) |

### B5. Licensing

| ID | Item | Evidence | Class |
|----|------|----------|-------|
| B5.1 | `LICENSE` Apache-2.0; `license.workspace = true` in members. | `LICENSE`; `Cargo.toml:14` | `as-is` |
| B5.2 | SPDX header on every Rust and Python source: `SPDX-License-Identifier: Apache-2.0` and `Copyright 2026 The Grimoire Authors`; enforced for Rust by hawkeye (`task rust:license:check`, `license:format`). | `.licenserc.toml:1-9` | `pattern` (holder name and year per repository; `includes` globs per layout; Tauri adds `src/**/*.ts` and `src-tauri/**/*.rs`) |
| B5.3 | `NOTICE` file: absent. Bundled licence texts live under `crates/grim-mirror/licenses/` as product data, not as a notice. | `ls` of the repository root | `grimoire-only` (negative finding) |
| B5.4 | Dependency licence audit (`cargo deny check licenses`) in `task verify` and the `supply-chain` job. | `rust.taskfile.yml` `license:deps`; `verify-basic.yml:50-80` | `as-is` |

### B6. Security tooling

| ID | Item | Evidence | Class |
|----|------|----------|-------|
| B6.1 | cargo-deny: licences in `task verify`, advisories and bans and sources in the PR `supply-chain` job; installed by `taiki-e/install-action`. | `verify-basic.yml:50-80` | `as-is` |
| B6.2 | `cargo audit bin` on the auditable-built launcher. | `rust.taskfile.yml` `hook:audit`; `verify-basic.yml` `hook-size` | `grimoire-only` |
| B6.3 | `npm audit --audit-level=high` inside `docs:check`. | `docs.taskfile.yml` `check` | `pattern` (Tauri front end: audit its own lockfile) |
| B6.4 | trivy, zizmor, gitleaks: no workflow, task or config mentions them; `product-tech-strategy.md` still lists Trivy under Security (drift). | a case-insensitive grep for the three names over `.github`, taskfiles, `deny.toml` is empty | `grimoire-only` (negative finding) |
| B6.5 | `SECURITY.md`: reporting channel, supported versions, guarantees, in-scope list, known limitations. | `SECURITY.md:1-100` | `pattern` |
| B6.6 | Threat model and trust rules (`arch-threat-model.md`, `adr_artifact_trust_model.md`). | `.claude/rules/arch-threat-model.md` | `grimoire-only` (other researcher; the mirror carries its own subset in `subsystem-mirror.md` invariants 3 to 5) |

### B7. Release

| ID | Item | Evidence | Class |
|----|------|----------|-------|
| B7.1 | `dist-workspace.toml`: cargo-dist 0.33.0, GitHub CI, shell and powershell installers, 8 targets, `dispatch-releases = true`, `cargo-auditable`, `cargo-cyclonedx`, GitHub attestations, `post-announce-jobs`, runner override `macos-15`; `grim-mirror` carries `dist = false`. | `dist-workspace.toml:1-50` | `pattern` |
| B7.2 | Version bump and ceremony: `task release:prepare` (cargo set-version across the workspace, regenerated `CHANGELOG.md`), `release: vX.Y.Z` commit, dispatch from main; skill `prepare-release`. | `taskfiles/release.taskfile.yml`; `.claude/rules/workflow-release.md` | `pattern` |
| B7.3 | Changelog flow: git-cliff builds `CHANGELOG.md` (Keep a Changelog), a separate site changelog page per release. | `cliff.toml`; `docs/src/content/docs/changelog.md` | `pattern` |
| B7.4 | `packaging/grim/`. | `ls packaging` | `grimoire-only` |

### B8. Documentation

| ID | Item | Evidence | Class |
|----|------|----------|-------|
| B8.1 | Docs site: Astro Starlight under `docs/` (`astro.config.mjs`, `package.json`, `package-lock.json`, `src/content/docs/`, `public/`), Node 24 from `ocx.toml`, deploy to GitHub Pages. | `docs/`; `docs.yml` | `pattern` |
| B8.2 | Page declarations: each page carries `<!-- doc_type: how-to -->` and `<!-- doc_tier: integration -->` after the frontmatter. | `docs/src/content/docs/mirroring.md:1-8` | `as-is` |
| B8.3 | `docs/docs.toml`: `product_shape = "cli"`, `discovery_artifact`. | `docs/docs.toml:1-6` | `pattern` (plugin crate: `cli`; Tauri: a shape from the `cli | library | hosted-service | framework` set that fits, to be decided) |
| B8.4 | Checks under `.claude/rules/docs-quality/checks/`: `doc_declaration.py`, `doc_examples.py`, `landing_check.py`, `links_raw.py`, `nav_depth.py`, `page_type.py`, `prose.py`, `strip_prose.py`, `lychee.toml`, `markdownlint.jsonc`, `vale.ini`, fixtures; vendored from `ocx-sh/grimoire-lore`, with two local edits listed in `.claude/rules.md`. | `.claude/rules.md` "Vendored rules"; `ls .claude/rules/docs-quality/checks` | `as-is` (vendored rule set; reapply the two local edits) |
| B8.5 | Site gates `docs/check_urls.py`, `docs/landing_check.py`, `docs/theme_check.sh`, `docs/migrate_frontmatter.py`. | `docs/` | `grimoire-only` |
| B8.6 | Docs style rules `docs-style.md` (Grimoire voice) and `docs-quality.md` (18 blocking rules). | `.claude/rules.md` | `pattern` (`docs-quality` as-is, `docs-style` rewritten) |
| B8.7 | Reference docs generated from code: JSON Schemas via `task schema:generate`, `CATALOG.md`, `docs/public/` casts. | `taskfiles/schema.taskfile.yml`; `CATALOG.md` | `grimoire-only` |

### B9. Repository files

| ID | Item | Evidence | Class |
|----|------|----------|-------|
| B9.1 | `README.md` sections: title, Install, Quick Start, Run Your Own Index, Development, Community, License. | `README.md:5-94` | `pattern` |
| B9.2 | `CONTRIBUTING.md` sections: Prerequisites, Layout, Building, Running Tests, Code Style, Commit Conventions, Branch Model, Before Submitting, License (DCO). | `CONTRIBUTING.md` | `pattern` (tests section says `cargo nextest run` and `task test`; the layout table is grim's) |
| B9.3 | `CODE_OF_CONDUCT.md`: Contributor Covenant 3.0, one contact address. | `CODE_OF_CONDUCT.md:1,46` | `as-is` |
| B9.4 | `.gitattributes`: `* text=auto eol=lf`, LFS filter for jpeg, jpg, png, gif, binary flags for archives, `-text` for `test/data/conformance/**` and `crates/grim-mirror/tests/licenses/*.crlf.txt`. | `.gitattributes` | `pattern` (the first two blocks as-is; the last two paths are grim's; Tauri adds icon and font patterns) |
| B9.5 | `.gitignore` (112 lines): sections for project, `target/`, `test/bin/`, `test/.venv/`, Claude local and state files, Claude hook state, reports. | `.gitignore:1-40` | `pattern` (Tauri adds `node_modules`, `src-tauri/target`, `dist`) |
| B9.6 | `.editorconfig`: absent. | `ls -a` | `grimoire-only` (negative finding) |
| B9.7 | `.gitmodules` for two patch submodules. | `.gitmodules` | `grimoire-only` |
| B9.8 | `ocx.toml` and `ocx.lock`: pinned dev toolchain (`zot`, `node`, `cargo-deny`, `hawkeye`, `lychee`, `ruff`, `shellcheck`, `shfmt`, `uv`, `git-cliff`) with group `g0` pinning `grim` 0.14.3; `task test` runs under `ocx exec`, CI uses `ocx-sh/setup-ocx`. | `ocx.toml:1-22`; `acceptance.yml` "Setup OCX" | `pattern` (the tool pin list per repository; the mirror needs `zot`, `uv`, `ruff` and a `grim` pin as the system under test) |
| B9.9 | `taplo.toml` (TOML schema enabled). | `taplo.toml` | `as-is` |
| B9.10 | Ruff configuration: absent (no `ruff.toml`, no root `pyproject.toml`); ruff 0.17 pinned in `ocx.toml` and run with defaults on `scripts/` only; `test/` is not linted by `task verify`. | `ls ruff.toml test/ruff.toml pyproject.toml` fails; `test/taskfile.yml` task `scripts` | `grimoire-only` (negative finding) |
| B9.11 | `scripts/` (`backfill_release.py`), `catalog/`, `CATALOG.md`, `grimoire.toml`, `grimoire.lock`, `.mcp.json`, `.serena/`. | repository root | `grimoire-only` |

## Negative findings

- No pull request template, no `NOTICE`, no `.editorconfig`, no `.cargo/config.toml`, no `rust-version` (MSRV) field.
- No renovate; dependabot only.
- No trivy, zizmor, gitleaks or CodeQL anywhere, although `product-tech-strategy.md` lists Trivy as the golden-path security tool.
- No signed-commit enforcement; only DCO sign-off.
- No ruff configuration; the acceptance tests under `test/` are not linted by `task verify` (only `scripts/` is).
- The mirror suites do not read `test/data/`; `test/data/publish_mirror/` belongs to grim's `publish.toml` `mirror` table test.
- Windows is not covered for `grim-mirror` at all (fixture skip, no build target).

## Leads

- `test/tests/conftest.py` imports `opentelemetry` through `src.otlp_fixture` at module top, so any copy that keeps the file drags in a grim-only dependency.
- `test/src/git_fixture.py:18` and `test/tests/test_mirror_bundles.py:19` read `crates/grim-mirror/licenses/MIT.txt` by repository-relative path; the path changes with the new layout.
- `crates/grim-mirror/tests/boundary.rs` reads `../../dist-workspace.toml` and runs `cargo tree` at the root; both disappear or change after the split.
- `task test:build` builds `grim` and `grim-mirror` from one tree; the new repository gets `grim` from a release or `ocx` (group `g0` shows the pattern), which has no pin for the current grim (0.17.0 is unreleased in this branch).
- `.claude/tests/test_ai_config.py` and the catalog drift check read the Rust and docs layout; they will fail in a copy until adapted (other researcher).
- Hawkeye `includes` (`.licenserc.toml`) list `src/**/*.rs`, `crates/**/*.rs`, `build.rs`, `build/**/*.rs` only, so a Tauri tree (`src-tauri/`, TypeScript) is outside the header gate unless the globs change.
- `task test:floor` refuses a park marker in `src`, `test` and `crates`; a new repository with a different layout needs its own `PARK_SCOPE`.
- Deep verify's Windows legs are grim-only; the owner's WSL host runs the Windows suite locally through `/mnt/c/Users/ecom/grim-wintest/run.sh` (user instructions), which has no `grim-mirror` build.
