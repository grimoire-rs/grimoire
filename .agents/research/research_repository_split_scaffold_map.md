# Research: Scaffold Copy Map for grim-mirror and grim-gui

## Metadata

**Date:** 2026-11-26
**Domain:** repository split, scaffold, CI, agent config
**Builds on:** `research_repository_split_agent_config.md`, `research_repository_split_testing_and_conventions.md`, `research_repository_split_recon.md` § 2 (not repeated)
**Method:** read-only; local files, no network, no builds. Source tree: `hex/artifact-mirroring` at `eaa7da0b`.

## Direct Answer

- Bundle names are confirmed on disk for all five: `rust-essentials`, `python-essentials`, `typescript-essentials`, `docs-essentials`, `code-docs-essentials` (`/home/mherwig/dev/ocx-sh/grimoire-lore/bundles/*.toml`). `hex` and `nox` are confirmed in `grimoire.toml:7-8`. Only the exact registry path of the lore bundles is inferred from the indexer lockfile pattern `ghcr.io/ocx-sh/lore/<name>:latest`; no pull was made.
- A host grim exists: `grim 0.16.1` on PATH via an ocx trampoline. grim 0.16.x is the version that wrote the current lock files (`generated_by` in the indexer lock says 0.13.0; this repo's lock is newer). A client list of `["claude"]` writes to `.claude/skills/<name>/` and `.claude/rules/`.
- Riskiest copy items: (a) `crates/grim-mirror` exists only on the feature branch, not on `main` (`72d3fce1`), so "pinned grimoire commit" for CI must be a `main` commit for grim and the mirror source must come from the split; (b) `test_ai_config.py` hard-requires 12+ grimoire-authored rule files; (c) grim's build needs two git submodules; (d) `pyproject`/`conftest` of `test/` is shaped for grim; (e) no workflow linter is installed.

## 1. Bundles and OCI refs

### Provenance on disk

| Artifact | Frontmatter `repository:` | Evidence |
|---|---|---|
| `docs-quality`, `rust-quality`, `rust-cargo`, `python-quality`, `python-packaging`, `typescript-quality`, `typescript-packaging`, `code-docs` | `https://github.com/ocx-sh/grimoire-lore` | `.claude/rules/docs-quality.md:16`, `rust-quality.md:7`, `rust-cargo.md:17`, `python-quality.md:10`, `python-packaging.md:8`, `typescript-quality.md:13`, `typescript-packaging.md:17`, `code-docs.md:62` |
| `hex-state` | `https://github.com/michael-herwig/arcana` | `.claude/rules/hex-state.md:5` |
| skills `code-docs-cleanup`, `docs-instrument`, `docs-plan`, `docs-review` | grimoire-lore | `.claude/skills/<name>/SKILL.md:8` (first one confirmed `ocx-sh/grimoire-lore`) |
| skills `hex-*`, `nox-review` | arcana | `.claude/skills/hex-plan/SKILL.md:8`, `nox-review/SKILL.md:8` |

The repo declares only hex and nox: `grimoire.toml:7-8` (`ghcr.io/michael-herwig/arcana/hex:latest`, `.../nox:latest`), `[skills]` and `[rules]` empty (`grimoire.toml:10-12`). `grimoire.lock:82-85` and `:143-146` pin both bundles. So the lore rules and skills in this repo were **vendored by copy**, not installed through `grimoire.toml` (the lock has no lore bundle).

### Bundle members (grimoire-lore, confirmed)

Source: `/home/mherwig/dev/ocx-sh/grimoire-lore/bundles/<name>.toml`, HEAD `afd517e0`, `[bundles.*]` entries in `publish.toml` (lines 162, 186, 223, 257, 283), `repository_prefix = "ocx-sh/lore"` flat (`publish.toml`, comment "Flat: every artifact publishes as ghcr.io/ocx-sh/lore/<name>").

| Bundle | Rules | Skills |
|---|---|---|
| `rust-essentials` | `rust-quality`, `rust-cargo`, `code-docs` (lines 21-25) | `code-docs-cleanup` (28) |
| `python-essentials` | `python-quality`, `python-packaging`, `code-docs` (19-23) | `code-docs-cleanup` (26) |
| `typescript-essentials` | `typescript-quality`, `typescript-packaging`, `code-docs` (19-23) | `code-docs-cleanup` (26) |
| `docs-essentials` | `docs-quality` (29) | `docs-plan`, `docs-instrument`, `docs-review` (32-34) |
| `code-docs-essentials` | `code-docs` (24) | `code-docs-cleanup` (27) |

Bundles include `code-docs` themselves, so a separate `code-docs-essentials` is redundant when a language bundle is present (`publish.toml:266-269` states code-docs is a member of every essentials bundle whose source it globs). Skills `hex-*` and `nox-review` come from arcana `hex` and `nox`. `hex-state` rides with the `hex` bundle (inferred: it is vendored from arcana and is a hex rule; the lock names hex members only as skills, confirm with `grim install` dry run).

### Refs a `grimoire.toml` would name

Indexer precedent (confirmed): `typescript-essentials = "ghcr.io/ocx-sh/lore/typescript-essentials:latest"` (`/home/mherwig/dev/grimoire-rs/indexer/grimoire.toml:5`; lock `bundle = "ghcr.io/ocx-sh/lore/typescript-essentials"`). Header line: `#:schema https://grimoire.rs/schemas/grimoire-config.schema.json`; `[options] clients = ["claude"]`.

Rust plugin repo (grim-mirror, has Python tests, uses docs):

```toml
[bundles]
hex = "ghcr.io/michael-herwig/arcana/hex:latest"                      # confirmed (grimoire.toml:7)
nox = "ghcr.io/michael-herwig/arcana/nox:latest"                      # confirmed (grimoire.toml:8)
rust-essentials = "ghcr.io/ocx-sh/lore/rust-essentials:latest"        # name confirmed, path inferred
python-essentials = "ghcr.io/ocx-sh/lore/python-essentials:latest"    # name confirmed, path inferred
docs-essentials = "ghcr.io/ocx-sh/lore/docs-essentials:latest"        # name confirmed, path inferred
```

`code-docs` arrives through `rust-essentials` and `python-essentials`; no extra entry.

Tauri repo (grim-gui):

```toml
[bundles]
hex = ...  nox = ...                                                  # as above
rust-essentials = "ghcr.io/ocx-sh/lore/rust-essentials:latest"
typescript-essentials = "ghcr.io/ocx-sh/lore/typescript-essentials:latest"   # confirmed pattern (indexer)
```

`code-docs` comes in with both. No `docs-essentials` unless the GUI repo grows a docs site. (Inferred: no `python-essentials` for GUI, since it has no pytest suite.)

### grim on this host

- `which grim` = `/home/mherwig/.ocx/toolchain/active/bin/grim`, a 221-byte shell trampoline that execs `ocx --global exec -- grim` (file contents read). `grim --version` prints `grim 0.16.1`. `grim install --help` runs ("Materialize the locked artifacts into the configured AI client(s)").
- Not run: `grim install` itself (read-only brief). Docs: skills land at `.claude/skills/<name>/SKILL.md` (`docs/src/content/docs/first-skill.md:67`, `quickstart.md:97`, `vendor-metadata.md:463`). Rules: `docs/src/content/docs/clients.md:714` says Qoder shares "the same config shapes as Claude" and that "skills, rules and agents install unchanged"; direct evidence for Claude rules is on disk: `.claude/rules/hex-state.md` carries arcana provenance and the lock installs hex. Dropped hex rule at `.claude/rules/` is therefore the observed layout.
- Gap: the lore rules carry depth subtrees (`.claude/rules/code-docs/` 284 files); bundles ship them whole, so the test-side `is_vendored` exemption (`test_ai_config.py:26-70`) still applies.

## 2. Taskfiles

Legend: M = mirror repo keeps, G = GUI repo keeps. "adapt" = keep with edits.

### Root `taskfile.yml` (102 lines)

| Lines | Item | M | G | Notes |
|---|---|---|---|---|
| 5-6 | `vars.REPORTS_ROOT` | yes | yes | as is |
| 9 | include `bench` | no | no | grim-hook fixture |
| 10-14 | include `git`, `rust`, `shell`, `coverage`, `release` | yes | yes | see per-file below |
| 15-17 | include `schema`, `docs`, `upstream` | no | no | grimoire docs site and upstream staleness |
| 18 | include `p9` | no | no | principle-9 diff |
| 19-21 | include `claude` (`dir: ./.claude`) | yes | yes | as is |
| 22-24 | include `test` (`dir: ./test`) | yes | no | GUI has no pytest suite (use `npm test`/cargo) |
| 25-27 | include `catalog` | no | no | |
| 30 `check` | `cargo check --workspace` | yes | yes | GUI: `--manifest-path src-tauri/Cargo.toml` |
| 33 `run` | runs grim | adapt | adapt | mirror: `cargo run -p grim-mirror`; GUI: `npm run tauri dev` |
| 37 `push`, 46 `checkpoint` | dev helpers | yes | yes | `push` conflicts with never-push rule; drop it if wanted |
| 53 `default` | fmt + clippy + check | yes | yes | |
| 61 `.ensure-cargo-tool` | shared helper | yes | yes | |
| 69 `verify` | lint, build-test, mark | yes | yes | |
| 87 `.verify:lint` deps | `rust:format:check`, `rust:clippy:check`, `shell:verify`, `claude:verify`, `test:scripts` | adapt | adapt | drop `test:scripts` unless a `scripts/` dir exists; GUI adds `npm run lint` and a type check |
| 96 `.verify:build-test` | `rust:build`, `catalog:verify`, `rust:test:unit`, `test:parallel` | adapt | adapt | drop `catalog:verify`; GUI replaces `test:parallel` with `npm test` |

Stamp lines (`taskfile.yml:80-85`), copy verbatim:

```yaml
  .verify:mark:
    internal: true
    desc: Mark commit-verified state for the pre-commit hook.
    cmds:
      - mkdir -p {{.ROOT_DIR}}/.claude/hooks/.state
      - date +%s > {{.ROOT_DIR}}/.claude/hooks/.state/commit-verified
```

### `taskfiles/rust.taskfile.yml` (137 lines)

| Lines | Task | M | G |
|---|---|---|---|
| 5-15 | `rust-sources` anchor lists `src/**`, `crates/**`, `build.rs`, `build/**` | adapt: mirror `src/**/*.rs`, `tests/**`; | adapt: `src-tauri/**/*.rs`, `src-tauri/Cargo.toml`, `src-tauri/build.rs` |
| 18 `verify` | fmt, clippy, `license:deps`, build, `test:unit` | yes | yes |
| 35-51 `format:*`, `clippy:*` | `--workspace --locked`; GUI needs `--manifest-path src-tauri/Cargo.toml` (or a root workspace) | yes | adapt |
| 53-59 `build` | `status: test -f target/release/grim` (line 57) | adapt to `grim-mirror` | adapt (Tauri build is `npm run tauri build`; drop the status test) |
| 61-72 `install:nextest`, `test:unit` | nextest, `--workspace --release` | yes | yes |
| 74-102 `license:*` (hawkeye, cargo-deny) | yes | yes |
| 109 `audit` | `cargo deny check advisories bans sources` | yes | yes |
| 118-137 `hook:size`, `hook:audit` | names `grim-hook` | no | no |

### Small files

- `shell.taskfile.yml` (31 lines): `verify` 6, `format` 23. Generic; copy as is (both). Test `test_shell_lint_handles_no_scripts` reads it (`test_ai_config.py` TestTaskfileLint 863+).
- `git.taskfile.yml` (55 lines): `merge` 9, `dco` 27; default range `main..HEAD` (line 30). As is, both.
- `release.taskfile.yml` (70 lines): `install:cargo-edit` 7, `changelog` 15, `changelog:preview` 21, `prepare` 36. Line 61 (`catalog/publish.toml`) and 67 (`/changelog` docs page) and 70 (`release.yml` dispatch) are grimoire-only: delete. Mirror: keep (crate version at the crate path). GUI: adapt `cargo set-version` (line 59) to `src-tauri/` plus `package.json` and `tauri.conf.json` version.
- `coverage.taskfile.yml` (43 lines): `install` 7, `default` 17, `open` 27. As is for both; GUI needs `--manifest-path`.
- `.claude/taskfile.yml` (83 lines): `verify` 23, `statusline:install` 29, `tests` 35, `fix:canonical-block` 47, `lint:links` 53. Keep all. Edit `lint:links` excludes (lines 63-78): `rules/rust-cargo/crates-of-record.md`, `skills/DESIGN\.md`, `rules/docs-quality/checks/fixtures` apply wherever those bundles install; add a `rules/code-docs` exclude if any link check fails after install (inferred, untested). `claude-sources` (lines 9-20) lists `../taskfiles/**/*.yml`, `../.github/workflows/**`, `../.agents/plans/*.md`, fine. Line 41 runs `test_ai_config.py test_upstream_stale.py`: drop `test_upstream_stale.py` (grimoire's script).
- `test/taskfile.yml` (264 lines), mirror only. Keep: `build` 12 (rewrite, see § 4), `default` 23, `parallel` 171, `floor:warn` 195, `shard` 204, `floor` 231, `quick` 163. Drop: `scripts` 37 (needs `scripts/`), `.index-e2e-indexer` 46, `index-e2e` 56, `index-matrix` 79, `old-grim` 103, `bench` 149, `demo` 243. The ten mirror suites must be checked against the module lists inside `old-grim` and `index-*` to prove none moved. Vars lines 5-9 (`GRIM_BINARY`, `GRIM_MIRROR_BINARY`) stay; `GRIM_BINARY` must point at a grim built from the pinned commit.

## 3. `.claude/hooks/*.py` and `.claude/tests/*.py` edits beyond the agent_config inventory

### Hooks

- `post_tool_use_tracker.py`: `CONTEXT_REMINDERS` is lines 31-45 (13 `src/...` rows, all grimoire subsystem rules; `subsystem-*.md` are not in a fresh copy). Replace by one row or empty the list (test `test_context_reminder_for_cli_subsystem` in `test_hooks.py:~717-722` then fails). `CONFIG_REMINDERS` 48-190: delete rows naming `catalog/` (79-83), `docs/src/content/docs/*.md` (85-117, 6 rows), `src/oci/hook.rs` (97-101), `src/install/vendor_*.rs` and `client_target.rs` (~118-143), `Cargo.toml` row (145-149) keeps with its AGENTS.md pointer, `.agents/adr/adr_*.md` (160-164 names arch-principles ADR Index), `.claude/rules/subsystem-*.md` (155-159), `.agents/upstream-checks.md` (181-189). Keep `taskfile.yml`, `taskfiles/*`, `test/taskfile.yml`, `.github/workflows/*`, `deny.toml`, `.claude/rules/*.md`, `.agents/plans/plan_*.md`. Rows point to `.claude/skills/builder/SKILL.md` and `qa-engineer` (lines 52-65), which are not in either repo unless copied. Line 70, 77: `subsystem-tests.md`, `subsystem-ci.md` are grimoire rules.
- `pre_commit_verification.py`: line 297 message ("format, clippy, lint, license, build, tests") generic; line 233 detects `Cargo.toml` in `root` only: the GUI repo's Rust lives in `src-tauri/Cargo.toml`, so tool detection reports "none detected" there (message only; the guard works from the stamp). Line 296 hand-stamp instruction contradicts the project note against hand-written stamps; keep for parity.
- `pre_tool_use_validator.py:69-73` `_GENERATED_FILES` names `release.yml` / cargo-dist: delete or keep if the repos use `dist`; then `test_hooks.py:660-673` (3 tests: `release.yml` generated, `verify-basic.yml` not, `src/main.rs` not) must change.
- `hook_utils.py:303,339,352`: references ADR `adr_ai_config_cross_session_learnings_store.md` and the category list `rust|python|ts|oci|test|clippy|mirror|build|other` (`oci` and `mirror` are grimoire categories; harmless). `subagent_stop_logger.py:7` cites the same ADR. Copy needs the ADR text or the citation dropped (`test_ai_config.py` Phase 4 asserts the citation, lines 1261-1300).
- `conventional_commit_validator.py`, `pre_push_main_blocker.py`, `session_start_loader.py`, `stop_validator.py`, `user_prompt_router.py`: no grimoire facts found by grep. `settings.json` hook wiring uses `uv run "$CLAUDE_PROJECT_DIR/.claude/hooks/<x>.py"`, as is.

### `test_ai_config.py` (2011 lines)

- 86-95 `VENDORED_RULES`, 98-114 `VENDORED_SKILLS`: must equal the installed set. Mirror: add `python-*`, drop `typescript-*` if the Python bundle is absent; GUI: drop `python-*`, `docs-*`. Mind `_EXPECTED_DISABLE_MODEL_INVOCATION` (1031-1062): any other vendored skill that appears makes line 1235-1244 fail.
- 181-192 `_GRIMOIRE_FORBIDDEN_STRINGS` (10 grim type names): inapplicable but harmless; trim. 194-198 `_SHAREABLE_RULES` names `quality-core/rust/python/bash.md`: those first-party rules do not exist unless copied; the plan decides copy or delete (agent_config inventory covers this).
- 444: iterates `subsystem-*.md` (empty glob passes). 480-482 and 23: AGENTS.md budget 280 lines (`test_line_budget`). 520-521: `grimoire-wt-` must appear in AGENTS.md; new repo names are `grim-mirror-wt-`, so edit the assertion or write the sentence with `grimoire-wt-`. 27-37: AGENTS.md must contain "N principles distill" with N headings.
- 534-562 `TestFeatureWorkflow` reads `workflow-feature.md`; 774-812 `TestReleaseImplementation` reads `workflow-release.md` and checks workflow filenames in it; 899-912 requires `workflow-bugfix.md`/`workflow-refactor.md` have `paths:`; 925-948 expects exactly **4** global rules (`len(globals_found) == 4`, line 935) and enumeration in `meta-ai-config.md` `### Current Global Rules`; 1286-1296 needs `meta-ai-config.md` with a "Cross-Session Learnings Store" section; 1305-1391 needs the Review-Fix Loop canonical block between `workflow-bugfix.md` and `workflow-refactor.md`; 1688-1770 `TestPlanStatusBlock`; 1815-2010 `TestAdrIndex` needs `arch-principles.md` with "## ADR Index" (skips when `.agents/adr/` has no `adr_*.md`, 1947-1949). So the test file requires these first-party rules: `meta-ai-config.md`, `workflow-bugfix.md`, `workflow-refactor.md`, `workflow-feature.md`, `workflow-release.md`, `arch-principles.md` (skipped only if no ADRs). Decide per file: copy rule, or delete the test class.
- 1426-1568 `TestPromptRoutingTriggers`, 1569-1687 `TestUserPromptRouter` read skill descriptions and `user_prompt_router.py`; they need the grimoire local skills (`builder`, etc.) unless pruned.
- Global count: with the arcana `hex-state` plus any first-party globals (`quality-core`, `product-tech-strategy`, `workflow-intent`) the count is 4 in grimoire; compute per repo.

### `test_hooks.py` (1398 lines)

Rows asserting grimoire facts: 660-673 (`release.yml`/cargo-dist), 716-722 (`src/command/install.rs` gives "CLI" reminder), 724-728 (taskfile reminder text "AI CONFIG UPDATE NEEDED", fine), 735-740 (`src/oci/**` glob test, pure logic, fine), 751-757 (`.claude/rules/quality-core.md` edit expects "catalog" in the reminder, so `quality-core.md` or any rule path works). Other `src/*.rs` lines are path-string fixtures. 52-67 reads `taskfiles/shell.taskfile.yml`. `test_workflows.py` (64 lines): grimoire-specific signing guards (`ALLOWED_ID_TOKEN` 17-23, `release-ocx.yml`): drop in both repos. `test_upstream_stale.py` (520 lines): drop, along with `.claude/taskfile.yml:41`.

## 4. CI carry-over

### `verify-basic.yml` (238 lines, name `Basic Verification`)

| Job (line) | Mirror | GUI | Change |
|---|---|---|---|
| `Conventional Commits` 21 (cocogitto v4.2.0; reads `cog.toml`) | yes | yes | as is |
| `DCO Sign-off` 37 (`task git:dco`) | yes | yes | as is |
| `Supply Chain` 57 (`task rust:license:deps`, `task rust:audit`) | yes | yes | drop `submodules: recursive` (line 68) unless the mirror pulls grim as a crate; remove comment about `external/docker_credential` |
| `Smoke (Linux)` 94 steps: Checkout, Setup Rust, Rust Cache, Setup Task, nextest, fmt, Clippy, Build, `Validate catalog` (128), `Build grim-mirror` (130), Test, Upload binaries (134, 141), Publish Unit Test Results, Check lint results | adapt | adapt | drop `Validate catalog`; mirror uploads `grim-mirror-binary` only; **add** steps that check out grimoire at a pinned SHA (with `submodules: recursive`), build grim (`cargo build --release --locked`) and upload it as `grim-binary` |
| `Acceptance Tests` 164 (calls `acceptance.yml` with inputs `binary-artifact`, `binary-file`, `mirror-artifact`, `mirror-file`, `runner-os`, `check-name`, `merged-artifact`) | yes | no | as is once `smoke` produces both artifacts |
| `Launcher Size` 180, `Launcher Size (Windows x64)` 222 | no | no | grim-hook |

GUI replaces `Smoke` with a Rust job under `src-tauri/` plus a Node job (`npm ci`, lint, type check, `vite build`); Tauri on Linux needs system libs (`libwebkit2gtk-4.1-dev` etc.; inferred from the Tauri 2 docs, not checked here).

### `acceptance.yml` (186 lines, reusable)

Jobs `plan` 52, `shards` 64, `merge` 131, `publish` 167. Steps carry over unchanged for the mirror. Edit points: `Download Binary` 80-84 and `Prepare Binary` 85-90 (artifact is grim built from the pinned commit; fine as is if the caller passes it), `Download grim-mirror` 91-103, `Setup OCX` 108-113 (pin `0.6.8`, group `default`: needs a mirror-repo `ocx.toml` with `zot`, `uv`), `Run Acceptance Shard` 114-120 calls `task test:shard`, `Suite Floor and Ceilings` 153-157 calls `task test:floor`. Shard count default 4 (line 47): the ten mirror suites may need fewer shards; `SUITE_FLOOR`, `SKIP_CEILING`, `XFAIL_CEILING` are files under `test/`.

### `verify-deep.yml` (workflow_dispatch only)

Jobs: `Build & Unit Test` 12 (Linux + macOS matrix, `build-rust` action), `Cross-compile` 56, `Launcher Size and Replace (macOS)` 92, `Hook Acceptance (Windows)` 130, `Acceptance` 187, `Platform Check (Windows)` 207, `Principle 9` 253, `Index end-to-end` 414, `Mod load probe` 561, `Publish Deep Test Results` 619. Mirror keeps `Build & Unit Test`, `Cross-compile` (if it ships to more targets), `Acceptance`, `Platform Check (Windows)` (review its 207-252 body before reuse), and `Publish Deep Test Results`; drop the hook, principle 9, index e2e and mod probe jobs. GUI keeps a build matrix only.

### Others

- `.github/actions/build-rust/action.yml` (40 lines): generic except `description` (line 2) and lines 33, 37-38 (`grim` names); mirror renames to `grim-mirror`; GUI does not use it (Tauri build differs). It uses `dtolnay/rust-toolchain@stable` instead of the pinned toolchain file: inconsistent with `rust-toolchain.toml` `1.95.0`; flag.
- `dependabot.yml` (43 lines): keep `github-actions` (lines 3-21; drop `exclude-paths: release.yml` 12-13 and the `build-rust` directory if unused) and `cargo` (22-31). Drop `npm /docs` (34-43) for mirror; GUI changes it to `npm /` and `cargo /src-tauri`.
- `claude.yml` (44 lines): generic, copy as is; needs secret `CLAUDE_CODE_OAUTH_TOKEN` (line 42).
- Linter: `actionlint` is **not installed**. The OCX index holds metadata only (`~/.ocx/index/ocx.sh/p/actionlint`); `command -v actionlint yamllint` returns nothing; `~/.ocx/symlinks/ocx.sh/` lists only astral-sh, embarkstudios, hawkeye, lychee, ocx, shellcheck, shfmt. Installing is outside this brief. `shellcheck` and `lychee` are installed.

## 5. Copy-as-is list

| File | Verdict | Lines to edit |
|---|---|---|
| `LICENSE` (201) | as is | none; copyright holder is named in `CONTRIBUTING.md:111` and `.licenserc.toml:3` |
| `CODE_OF_CONDUCT.md` (83) | as is | contact email at line 46 is the owner's; keep |
| `.taskrc.yml` (2) | as is | `interactive: true`, `failfast: true` |
| `rustfmt.toml` (1) | as is | `max_width = 120` |
| `rust-toolchain.toml` (4) | as is | channel `1.95.0`; GUI may need a newer Tauri-compatible MSRV (inferred, check at scaffold) |
| `taplo.toml` (2) | as is | `[schema] enabled = true` |
| `cog.toml` (5) | as is | `release` commit type; keep if the `release:` convention is kept |
| `deny.toml` (37) | edit | comment at lines 24-29 names grim's `oci-client` transitive advisories: delete or retarget; `unmaintained = "workspace"` stays; re-run `cargo deny` (GUI's Tauri tree adds licences such as `MPL-2.0`, already allowed, and probably more) |
| `.licenserc.toml` (5) | edit | line 3 "The Grimoire Authors"; line 5 `includes` lists `src/**/*.rs`, `crates/**/*.rs`, `build.rs`, `build/**/*.rs`; GUI: `src-tauri/**/*.rs`; mirror: `src/**/*.rs`, `tests/**/*.rs` |
| `.config/nextest.toml` (20) | edit | lines 15-20 override `test(/probe/) | test(/index_sync::tests::/)` is grim-only: delete; the `ci` profile (lines 7-13) as is; GUI: same file |
| `.gitattributes` (19) | edit | line 17-19: delete `crates/grim-mirror/tests/licenses/*.crlf.txt -text` (path moves to `tests/licenses/...` in the mirror) and `test/data/conformance/** -text` (cross-repo conformance tree, needed in mirror only if the tree moves) |
| `.gitignore` (112) | edit | line 2 title; 91-103 docs-site lines (`docs/dist`, `docs/public/schemas`) drop; add `target/`, `test/bin/`, `test/results/` if missing; GUI adds `node_modules/`, `dist/`, `src-tauri/target/`, `src-tauri/gen/` (inferred); keep `.claude/hooks/.state/`, `.agents/worktrees/` |
| `CONTRIBUTING.md` (118) | edit | line 1 title; layout table 12-20 names `src/`, `test/`, `taskfiles/`; lines 23-50 commands (`task test`, `task test:quick`, `grim` binary); 86 and 111 licence naming |
| `SECURITY.md` (120) | rewrite | lines 28-100 are grim's trust statement; keep sections 1-27 (reporting) and rewrite the rest for the repo's attack surface; links at 116-120 point to `grimoire-rs/grimoire` security tab and `grimoire.rs/clients.html` |
| `.github/CODEOWNERS` | as is | `* @michael-herwig` |
| `.github/ISSUE_TEMPLATE/bug_report.yml` | edit | lines 19, 28, 31 (`grim ...`, `grim: 0.1.0`), 39 (`GRIM_LOG`) |
| `.github/ISSUE_TEMPLATE/feature_request.yml` | check | no `grim` string found by grep; likely as is |
| `.github/ISSUE_TEMPLATE/config.yml` | edit | line 4 discussions URL `grimoire-rs/grimoire/discussions` to the new repo |
| `.claude/templates/**` | as is | 11 files under `templates/artifacts/`; no `grim` string found (grep clean); `lint:links` excludes them (`taskfile.yml:73`) |
| `.claude/scripts/**` | edit | `README.md:3` and `statusline-install.sh:2` say "Grimoire"; `sync-canonical-blocks.py` needs the canonical block in `workflow-bugfix.md`/`workflow-refactor.md`, otherwise drop it and `fix:canonical-block` (`.claude/taskfile.yml:47`) |

## 6. Existing paths and the main checkout

- `/home/mherwig/dev/grimoire-rs/grim-mirror`: does not exist (`ls`: no such file).
- `/home/mherwig/dev/grimoire-rs/grim-gui`: does not exist.
- `/home/mherwig/dev/grimoire-rs/grimoire`: exists, branch `main`, `git status --short` empty (clean), HEAD `72d3fce1 release: v0.17.0`; `main` equals `origin/main` (both `72d3fce1...`). Submodules populated: `external/docker_credential` and `external/rust-oci-client` hold files. Its `crates/` has only `grim-hook` and `grimoire-hook-core`: **`crates/grim-mirror` is absent on `main`**. `eaa7da0b` (this branch, `hex/artifact-mirroring`) is contained only in `hex/artifact-mirroring` (`git branch -a --contains`), merge base with `main` is `72d3fce1`. A CI step that builds grim from a pinned `main` commit works today; the mirror source must be moved by the split, not fetched from `main`.

## Negative results

- No actionlint, yamllint or other workflow linter is installed; no `grim` binary newer than 0.16.1 found.
- No lore bundle appears in this repo's `grimoire.toml` or `grimoire.lock`; no OCI pull was done, so the registry paths of the lore bundles are inferred from the indexer lock.
- No `hex-state` membership confirmation inside the `hex` bundle (the lock lists skills; rule membership not shown).
- No local checkout of a `lore` repo exists under another name; only `/home/mherwig/dev/ocx-sh/grimoire-lore` (clean except untracked `.agents/retro/inbox/`, `.serena/`).
- `user_prompt_router.py`, `session_start_loader.py`, `stop_validator.py`, `conventional_commit_validator.py`, `pre_push_main_blocker.py`: no grimoire-specific string found by grep.
- Did not run `grim install` or any test; the claim that a clean install satisfies `test_ai_config.py` is untested.
