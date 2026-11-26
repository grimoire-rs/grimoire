# Research: Repository Split Recon (grim-mirror, GUI Repository, Shared Base)

## Metadata

**Date:** 2026-11-25
**Domain:** packaging
**Triggered by:** Discussion about moving `crates/grim-mirror` into its own repository and starting a GUI application repository whose base is the same as this one. This document is an inventory only and makes no recommendation.
**Expires:** 2027-02-25

## Direct Answer

Facts only, per question. Paths are relative to `/home/mherwig/.herdr/worktrees/grimoire/uno` (branch `hex/artifact-mirroring`, workspace version 0.17.0) unless they start with `/home/mherwig/dev/grimoire-rs/`. Line counts come from `git ls-files` plus `wc -l`, so they cover tracked files only.

## 1. Coupling of `crates/grim-mirror` to the workspace

### 1.1 What it inherits from the root `Cargo.toml`

- `version.workspace`, `edition.workspace`, `license.workspace` (`crates/grim-mirror/Cargo.toml:3-5`). The root sets version `0.17.0`, edition `2024`, license `Apache-2.0` in `[workspace.package]` (`Cargo.toml:11-14`). `cargo set-version` in the release tasks bumps the whole workspace from that one site (`Cargo.toml:9-10` comment).
- `[lints] workspace = true` (`crates/grim-mirror/Cargo.toml:37`). The root lint tables are `unsafe_code = "forbid"`, clippy `unwrap_used = "warn"` and `expect_used = "warn"` (`Cargo.toml:~175-185`).
- No `workspace.dependencies` table exists. Every dependency version is spelled in the member's own manifest (`clap`, `serde`, `serde_json`, `toml`, `regex`, `globset`, `semver`, `sha2`, `time`, `tempfile`, `tokio`, `spdx`; `crates/grim-mirror/Cargo.toml:15-27`).
- Profiles: nothing is set in the member. It uses the workspace `release`/`dev` defaults. `[profile.dist]`, `[profile.hook]` and `[profile.hook-dev]` live in the root and serve grim and `grim-hook`. The mirror manifest carries `publish = false` and `[package.metadata.dist] dist = false` (`:8`, `:33-34`).
- Shared lockfile: one root `Cargo.lock` covers all members. `cargo build --locked -p grim-mirror` is used in CI (see 1.2).
- `[patch.crates-io]` (root `Cargo.toml`) points `docker_credential` and `oci-client` at `external/` submodules. The mirror does not depend on either crate, so the patches do not reach it.
- Toolchain and tool config applied from the repository root: `rust-toolchain.toml` (channel `1.95.0`, rustfmt and clippy), `rustfmt.toml` (`max_width = 120`), `clippy.toml` (disallows two `ratatui` types, irrelevant to the mirror), `deny.toml`, `.config/nextest.toml` (`ci` profile with retries; `.config` has 1 file, 20 lines).

### 1.2 References outside `crates/grim-mirror/`

Files that name the crate (found with `grep -rIl "grim-mirror\|grim_mirror"`):

| Place | What it does | Evidence |
|---|---|---|
| `Cargo.toml` | `members` lists `crates/grim-mirror` | `Cargo.toml:5` |
| `.gitattributes` | `crates/grim-mirror/tests/licenses/*.crlf.txt -text` | `.gitattributes:19` |
| `.github/workflows/verify-basic.yml` | builds `cargo build --release --locked -p grim-mirror`, uploads artifact `grim-mirror-binary`, passes `mirror-artifact`/`mirror-file` to the acceptance workflow | `:130-138`, `:171-172` |
| `.github/workflows/verify-deep.yml` | per-target `cargo build --release --locked -p grim-mirror --target=...`, uploads `grim-mirror-<target>`, passes it to acceptance | `:32-40`, `:200-201` |
| `.github/workflows/acceptance.yml` | `mirror-artifact` and `mirror-file` inputs; downloads the artifact, renames to `test/bin/grim-mirror`, `chmod +x` | `:24-29`, `:91-103` |
| `test/taskfile.yml` | `GRIM_MIRROR_BINARY` var, `cargo build --release --locked -p grim-mirror`, copy to `test/bin/` | `:8`, `:18-19` |
| `test/conftest.py` | `grim_mirror_binary` fixture and `GrimMirrorRunner` class | `:192-258` |
| `test/src/git_fixture.py` | reads `crates/grim-mirror/licenses/MIT.txt` at import | `:18` |
| `taskfiles/*.yml`, `taskfile.yml` | not named. `rust:clippy:check` and `rust:test:unit` use `--workspace` (`taskfiles/rust.taskfile.yml:47`, `:72`), `task check` uses `cargo check --workspace` (`taskfile.yml:~36`), coverage uses `--workspace` (`taskfiles/coverage.taskfile.yml:21`). The mirror's unit tests and clippy therefore ride the workspace commands. |
| `dist-workspace.toml` | not named (`members = ["cargo:."]`). `crates/grim-mirror/tests/boundary.rs:54-56` asserts it never names the crate. |
| `deny.toml` | not named. It applies workspace-wide (licenses allow list, `unknown-git = "allow"`). |
| `docs/src/content/docs/` | `mirroring.md` (the how-to), `commands.md`, `publishing.md`, `artifacts.md`, `json-interface.md`, `stability.md` (`:62-68`, `:662` Unstable sibling surface) |
| `catalog/skills/grim-usage/SKILL.md:145`, `catalog/skills/grim-authoring/references/release-checklist.md` | mention plugin dispatch and the mirror |
| `.claude/rules/subsystem-mirror.md` (path-scoped rule for the crate), `.claude/rules.md`, `.claude/rules/arch-principles.md:21,109`, `.claude/rules/product-context.md:106-110`, `.claude/rules/subsystem-cli-commands.md:44`, `.claude/rules/arch-threat-model.md` | rule catalog and subsystem text |
| `AGENTS.md:75-76` | layout paragraph: "`crates/grim-mirror` (separate binary, no dependency on the main crate, source-only, Unstable)" |
| `.agents/` | `adr/adr_artifact_mirroring.md`, `plans/plan_artifact_mirroring.md`, `specs/design_artifact_mirroring.md`, `goals/artifact-mirroring.md`, `upstream-watchlist.md:358-365`, `memory/hex.md:123,130` |

### 1.3 Textual couplings inside the crate that point back into the workspace

- `crates/grim-mirror/src/exit_code.rs:34`: `include_str!(".../../../src/cli/exit_code.rs")` and a test asserting each variant equals grim's of the same name (doc comment at `:5`).
- `crates/grim-mirror/src/git.rs:1019`: `include_str!(".../../../src/catalog/git_cmd.rs")`, a test that pins the copied git hardening argument and environment set against grim's file.
- `crates/grim-mirror/tests/boundary.rs:11-13` resolves `root()` as `CARGO_MANIFEST_DIR/../..` and reads `dist-workspace.toml` there (`:54-56`), runs `cargo tree -p grim-mirror -e normal --offline` from that root (`:19-35`), and checks the manifest has `publish = false` and `dist = false` (`:42-50`).
- `test/src/git_fixture.py:18` reads `crates/grim-mirror/licenses/MIT.txt` from the test harness. This is a harness-to-crate dependency in the opposite direction.

Size of the crate: 127 tracked files, 23,694 lines. `src/` has 16 `.rs` files (about 11,500 lines including inline unit tests; `run.rs` 2,119, `marketplace.rs` 1,148, `git.rs` 1,134, `publish.rs` 1,095, `license.rs` 1,023), `licenses/` 16 SPDX text files plus 92 files under `tests/` (license fixtures and `boundary.rs`), `examples/mirror.toml`, `README.md`.

### 1.4 Mirror tests under `test/`

- Ten files named `test/tests/test_mirror_*.py`, 71 test functions, 1,445 lines in total: `bundles` (128), `cli` (156), `dialect` (108), `kinds` (144), `license` (102), `marketplace` (374), `monorepo` (107), `retire` (100), `staging` (108), `versions` (118).
- Related but grim-side, not mirror tests: `test/tests/test_publish_mirror_table.py` and `test/data/publish_mirror/` (3 entries: `rules/`, `skills/`, `golden_no_table.json`). They cover the `mirror` table of `grim publish`, so they stay with grim.
- Fixtures the mirror tests use (from test signatures): `grim_mirror` (45 uses), `tmp_path`, `unique_repo`, `registry`, `grim_at`, `project_dir`. Imports: `src.git_fixture` (`GitRepo`, `BASE_DATE`, `skill_md`, `agent_md`, `monorepo`, `marketplace`), `src.helpers` (`write_config`, `make_artifact`), `src.registry` (`list_tags`, `fetch_manifest`, `fetch_blob`).
- `test/src/git_fixture.py` (199 lines) is mirror-only by its docstring ("Deterministic local git upstreams for the grim-mirror acceptance tests"). `src/helpers.py` (816 lines), `src/registry.py` (359) and `conftest.py` (362) are shared with the whole suite.
- Binary location: `grim` = env `GRIM_COMMAND`, else `test/bin/grim` (`.exe` suffix on Windows) (`test/conftest.py:~176-188`). `grim-mirror` = env `GRIM_MIRROR_COMMAND`, else `test/bin/grim-mirror`; skipped on Windows because no Windows build exists (`:192-206`). `GrimMirrorRunner` sets `GRIM_BIN` to the grim under test on top of the isolated `HOME`/`GRIM_HOME` environment from `src.runner.GrimRunner`, and offers `run` (direct) and `run_via_grim` (`grim mirror ...` with the mirror's directory first on `PATH`) (`:209-254`).
- The registry for these tests is a private `zot` started by `conftest.py` (or `GRIM_TEST_REGISTRY_HOST`), see `test/src/zot.py` (139 lines) and `conftest.py:109-128`.

### 1.5 How the mirror calls grim at run time

All calls go through `crates/grim-mirror/src/publish.rs` (`GrimDriver`, `run`, `run_retry`).

- Binary: `GRIM_BIN` when set and absolute, else `grim` on `PATH` (`resolve_bin`, `publish.rs:72-77`; read at `run.rs:56`). `GRIM_OFFLINE` is read at `run.rs:57` and the run refuses with a message at `run.rs:88` when it is truthy.
- Subcommands and flags:
  - `grim --version` (output parsed by stripping the prefix `grim `) (`publish.rs:~225-233`).
  - `grim schema --kind publish`, JSON parsed to confirm a `mirror` table exists (`:236-245`); otherwise the run fails with "missing the publish entry `mirror` table".
  - `grim convert <path> --from claude --kind <kind> --out <dir> --format json` (`:~282-296`).
  - `grim publish --format json [--dry-run] ...`, run in the staged group directory with an environment list (`:~342-351`).
  - `grim describe <ref> --format json`, twice (probe and annotation read) (`:~370-411`).
- Output formats: JSON only (`--format json`), parsed as `serde_json::Value` with field lookups, not typed structs (`publish.rs:296`, `:411`, `:428`, `:444`).
- Process handling: stdin null, piped stdout and stderr, a per-call deadline that kills the child and reports code 75, a retry on ETXTBSY (errno 26), one `run_retry` wrapper (`publish.rs:~100-181`). A `shortcut:` comment records that descendants are not killed.
- Other external program: `git`, run with hardened args and environment (`GIT_TERMINAL_PROMPT=0`, `GIT_ALLOW_PROTOCOL`, `protocol.ext.allow=never`, and so on), copied from `src/catalog/git_cmd.rs` (`.claude/rules/subsystem-mirror.md:15`).
- The reverse direction, `grim mirror <args>`, is grim's PATH plugin dispatch: unknown top-level names resolve `grim-<name>` on absolute `PATH` entries, built-ins win, and grim sets `GRIM_BIN` (plus `GRIM_OFFLINE`/`GRIM_LOCKED` when the flags came before the name) (`AGENTS.md` environment table, `GRIM_BIN` row).

## 2. The non-product "base" of this repository

Sizes are tracked files and lines. "Specific" means the content names grimoire concepts or paths; "generic" means it would carry over unchanged to another repository.

| Piece | Size | Specific or generic |
|---|---|---|
| `taskfile.yml` (root) | 1 file, 102 lines | Mixed. Structure (includes, `verify` two-phase gate, `.verify:mark` stamp for the commit hook, `.ensure-cargo-tool`) is generic. The includes `bench`, `schema`, `p9`, `upstream`, `catalog`, `docs` are grimoire-specific. |
| `taskfiles/` | 10 files, 1,082 lines (`rust`, `shell`, `git`, `release`, `coverage`, `bench`, `schema`, `docs`, `upstream`, `p9`) | `rust`, `shell`, `git`, `release`, `coverage` generic in shape. `bench` (references `grim-hook` build and a fixture), `schema`, `upstream`, `p9` (builds a main checkout under `target/p9/src` to diff), `docs` grimoire-specific. |
| `.github/workflows/` | 9 files, 2,011 lines: `verify-basic` (238), `verify-deep` (634), `acceptance` (186), `release` (353, cargo-dist output), `release-ocx` (87), `publish-catalog` (225), `docs` (115), `backfill` (129), `claude` (44) | `verify-*` and `acceptance` are shaped by grim and the mirror binary and the 8 release targets. `release` is generated by cargo-dist (`dist-workspace.toml`). `release-ocx`, `publish-catalog`, `backfill` are grimoire-specific. `claude` generic. |
| `.github/actions/` | 2 composite actions, 383 lines: `build-rust` (40), `release-ocx` (343) | `build-rust` generic. `release-ocx` grimoire-specific. |
| `.claude/` total | 594 files, 73,620 lines | see rows below |
| `.claude/rules/` | 444 files, 26,574 lines; 45 top-level rule files (6,760 lines) plus vendored rule subtrees (`code-docs/` 284 files, `docs-quality/` 68, `python-quality/` 12, `typescript-quality/` 12, `rust-quality/` 18, `rust-cargo/` 1) | Per `.claude/rules.md` "Vendored rules": nine rules come from `ocx-sh/grimoire-lore` (`docs-quality`, `rust-quality`, `rust-cargo`, `python-quality`, `python-packaging`, `typescript-quality`, `typescript-packaging`, `code-docs`) and `michael-herwig/arcana` (`hex-state`). First-party shareable rules are `quality-core`, `quality-rust*`, `quality-python`, `quality-bash`, `quality-security` (AGENTS.md calls them "shareable, project-independent"). Grimoire-specific: `subsystem-*` (17 files), `arch-*` (2), `product-*` (2), `workflow-*` (7), `vendor-capability-watchlist`, `meta-ai-config`, `docs-style`. |
| `.claude/rules.md` | 1 file | Catalog of every rule, with path-glob tables and declared overlaps. Grimoire-specific content over a generic structure. A structural test fails when it drifts. |
| `.claude/skills/` | 112 files, 37,155 lines | 15 vendored skills (`code-docs-cleanup`, `docs-instrument`, `docs-plan`, `docs-review`, `hex-*` x10, `nox-review`) per `.claude/rules.md`. Project-local: `bugfix`, `builder`, `changelog`, `code-check`, `commit`, `docs`, `finalize`, `fix`, `meta-maintain-config`, `meta-validate-context`, `next`, `outside-pr-review`, `prepare-release`, `qa-engineer`, `rca`, `security-auditor`, `upstream-refresh`. |
| `.claude/hooks/` | 10 files, 2,214 lines (`pre_commit_verification.py`, `conventional_commit_validator.py`, `pre_push_main_blocker.py`, `pre_tool_use_validator.py`, `session_start_loader.py`, `stop_validator.py`, and others) | Generic in design. The commit guard needs the `.claude/hooks/.state/commit-verified` stamp written by `task verify` (`taskfile.yml` `.verify:mark`). |
| `.claude/tests/` | 6 files, 4,185 lines (`test_ai_config.py`, `test_hooks.py`, `test_upstream_stale.py`, `test_workflows.py`, `pyproject.toml`, `uv.lock`) | Structural tests over rules, catalog, hooks and workflows. `test_workflows.py` and `test_upstream_stale.py` are grimoire-specific. |
| `.claude/templates/` | 15 files, 2,660 lines (artifact templates for ADR, plan, spec, research and so on) | Generic. |
| `.claude/taskfile.yml`, `.claude/scripts/` | 1 + 4 files | `claude:verify` and the sync/statusline scripts; generic. |
| `.claude/settings.json` | 1 file | Hook wiring. |
| `AGENTS.md`, `CLAUDE.md` | AGENTS.md about 280 lines; CLAUDE.md is a pointer that imports it | AGENTS.md is grimoire-specific (environment table, principles are generic). |
| `grimoire.toml` / `grimoire.lock` | 12 and 152 lines | `grimoire.toml` declares `[options] clients = ["claude"]`, bundles `hex = "ghcr.io/michael-herwig/arcana/hex:latest"` and `nox = "ghcr.io/michael-herwig/arcana/nox:latest"`, empty `[skills]` and `[rules]`. The lock pins the hex skills (`hex-architect`, `hex-core`, `hex-discuss`, `hex-execute`, `hex-finalize`, `hex-init`, `hex-loop`, `hex-plan`, `hex-retro`, `hex-review`) and `nox-review` by digest. Generic content. |
| `ocx.toml` / `ocx.lock` | 18 and 159 lines | Tool pins through OCX: `zot`, `node` 24, `cargo-deny`, `hawkeye`, `lychee`, `ruff`, `shellcheck`, `shfmt`, `uv`, `git-cliff`, plus group `g0` pinning `grim` 0.14.3 for the index end-to-end test. Mostly generic, the `g0` group is specific. |
| Release tooling | `dist-workspace.toml` (49), `cliff.toml` (91), `cog.toml` (5), `packaging/` (1 file, 14 lines), `scripts/` (2 files, 793 lines), `build.rs`, `build/` (1 file, 493 lines) | `dist-workspace.toml` is grimoire-specific: cargo-dist 0.33.0, 8 targets, shell and powershell installers, `dispatch-releases = true`, post-announce jobs `./release-ocx` and `./publish-catalog`, `members = ["cargo:."]`. `cliff.toml` generic in shape. |
| `deny.toml` | 37 lines | Generic: license allow list of 12 SPDX ids, `unmaintained = "workspace"`, `unknown-git = "allow"`. One comment names the `oci-client` transitive advisory. |
| `rustfmt.toml`, `clippy.toml`, `rust-toolchain.toml`, `taplo.toml`, `.licenserc.toml`, `.taskrc.yml` | 1, 4, 4, 2, ?, 2 lines | `rustfmt` (max_width 120), toolchain `1.95.0` generic. `clippy.toml` names ratatui types, grimoire-specific. `.taskrc.yml` is `interactive: true`, `failfast: true`. |
| Docs site scaffold | `docs/` 91 files, 48,690 lines (`astro.config.mjs`, `package.json`, `package-lock.json`, `docs.toml`, `check_urls.py`, `landing_check.py`, `theme_check.sh`, `migrate_frontmatter.py`, `public/` 23, `src/` 59) | Astro/Starlight scaffold generic. Content pages (`src/content/docs/`) grimoire-specific. `docs/node_modules` and `docs/dist` exist on disk (untracked). Needs Node 24 (`AGENTS.md` build section). |
| Acceptance-test harness | `test/` 567 files, 139,089 lines tracked: `test/src/` 15 modules (3,528 lines), `conftest.py` 362, `tests/` 247 files (114,823 lines), `data/`, `manual/`, `tools/`, `recordings/` | Harness (`runner.py` 167, `registry.py` 359, `zot.py` 139, `helpers.py` 816, `otlp_fixture.py`, `oci_proxy.py`, `consent_driver.py`, `hook_launcher.py`, `index_fixture.py` 446, `old_grim.py`) is partly generic (runner, registry, zot, oci_proxy) and partly grim-specific (consent, hook launcher, index fixture, scenarios). `pyproject.toml` pins Python >= 3.10, pytest, pytest-xdist, pexpect, rich. Ratchet files `SUITE_FLOOR`, `SKIP_CEILING`, `XFAIL_CEILING`. |
| Licence, DCO, community files | `LICENSE` (201, Apache-2.0), `CONTRIBUTING.md` (118), `SECURITY.md` (120), `CODE_OF_CONDUCT.md` (83), DCO check via `task git:dco` | Generic apart from names. |
| Git submodules | `.gitmodules` lists `external/docker_credential` (branch `feat/store-erase-list`) and `external/rust-oci-client` (branch `ocx/integration`), both `github.com/ocx-sh`; `git ls-files external` shows 2 gitlinks | Specific to grim; the mirror does not use either. |
| Planning records | `.agents/` 656 files, 174,268 lines (`adr/`, `plans/`, `specs/`, `research/`, `discussions/`, `goals/`, `memory/hex.md`, `upstream-watchlist.md`) | Specific to grimoire decisions; structure generic. |
| Catalog | `catalog/` 49 files, 10,534 lines, `catalog/taskfile.yml` | Specific (first-party packages). |

Totals for orientation: `src/` 326 files, 366,552 lines; `crates/` includes `grim-mirror` (23,694 lines) plus the two hook crates.

## 3. How `vscode` and `indexer` carry that base today

Both are checkouts at `/home/mherwig/dev/grimoire-rs/`, branch `main`, working tree read-only inspected with `git ls-files` and `diff`. Remotes: `git@github.com:grimoire-rs/grimoire-vscode.git` and `git@github.com:grimoire-rs/indexer.git`.

### 3.1 `vscode` (436 tracked files, 215 commits, TypeScript, npm)

- Present as copies of grimoire files. Byte-identical: `LICENSE`, `.taskrc.yml`. Diverged: `taskfile.yml` (94 differing lines against grimoire's), `taskfiles/git.taskfile.yml` (34), `taskfiles/release.taskfile.yml` (44), `.github/workflows/release.yml` (406, different generator), `.github/dependabot.yml` (55), `.gitattributes` (21), `cliff.toml` (19), `.claude/rules/quality-core.md` (55), `quality-security.md` (143), `quality-bash.md` (16), `workflow-git.md` (100), `.claude/skills/finalize/SKILL.md` (63).
- Own content with no grimoire counterpart: `.claude/agents/worker-*.md` (9), `.claude/skills/swarm-plan|swarm-execute|swarm-review/` (each `SKILL.md`, `classify.md`, `overlays.md`, `tier-{low,high,max}.md`), `.claude/rules/{artifact-kinds,grim-compat-markers,plan-status,quality-typescript,workflow-swarm}.md`, `.claude/artifacts/` (4 records), `.github/workflows/ci.yml` (137 lines), `.vscode/` (4), `.editorconfig`, eslint, prettier, esbuild, `assets/`.
- No sharing mechanism: no `grimoire.toml`, `grimoire.lock`, `ocx.toml`, `.gitmodules`, `uses: ./` or reusable workflow (`workflow_call`) was found. The swarm skills are an older in-repo generation, not the `hex-*` bundle grimoire installs. `taskfile.yml` is "a thin wrapper for the swarm/finalize skills" (`CLAUDE.md`), builds stay npm-driven.
- `CLAUDE.md` (70 lines) and `AGENTS.md` (12 lines) both exist; AGENTS.md says "Claude Code: read CLAUDE.md (authoritative)". This is the reverse of grimoire's arrangement (AGENTS.md authoritative).
- How it consumes grim: spawns the binary only from `src/grim.ts` via `execFile`, no shell, always `--format json`, parses an envelope (`error` key first, then exit codes) (`CLAUDE.md` Architecture section). Scope discovery via `grim context`. Version floors in `src/installer.ts`: `MINIMUM_GRIM_VERSION = '0.11.0'`, `REGISTRY_EDIT_GRIM_VERSION = '0.13.0'`, `RATING_GRIM_VERSION = '0.14.0'`, `PAGING_GRIM_VERSION = '0.15.0'`. It auto-installs grim from `https://github.com/grimoire-rs/grimoire/releases/latest/download` by reading cargo-dist's `dist-manifest.json` (`installer.ts:12-16`, `:149-156`, `:257`, `:301`). CI (`ci.yml:18`, `:60-72`) builds grim from source at a pinned commit (`GRIM_COMMIT: 84382946504fdd27dae166deb2b14ca2b53cbcf6`) with `cargo build --locked --release -p grimoire` and caches it, for a live test suite (`src/test/grimLive.test.ts`). The rule `.claude/rules/grim-compat-markers.md` defines `grim-polyfill<X.Y.Z` markers. No JSON schema files are tracked (`git ls-files | grep -i schema` found none).

### 3.2 `indexer` (563 tracked files, TypeScript, npm plus ocx)

- Uses a manifest: `grimoire.toml` (11 lines) declares `[bundles] typescript-essentials = "ghcr.io/ocx-sh/lore/typescript-essentials:latest"` and `[rules] css-theming = "ghcr.io/ocx-sh/lore/css-theming:latest"`, `clients = ["claude"]`. `grimoire.lock` (41 lines) pins `css-theming`, `typescript-packaging`, `typescript-quality` and the bundle by digest. Installed rule files sit tracked under `.claude/rules/` and are byte-identical to the grimoire copies for `typescript-quality/*` (12 files); `typescript-quality.md` differs in 7 lines and `typescript-packaging.md` in 6 lines from grimoire's copy.
- `ocx.toml` (32 lines) pins tools: `go-task`, `grim` (`ocx.sh/grimoire/cli:latest`), `actionlint`, `lychee`, `gitleaks`, `git-cliff`, with node 22 and node 24 as separate groups. `ocx.lock` present.
- Other copies: `LICENSE` identical; `.taskrc.yml` differs in 4 lines; `cliff.toml` 72 lines differ; `taskfile.yml` 242 differing lines (168-line file); `taskfiles/release.taskfile.yml` 84; `.github/workflows/release.yml` 397; `.gitattributes` 22; `.claude/settings.json` 98; `.claude/rules.md` 225 (own catalog). Own content: `ci.yml` (170), `bench.yml`, `pages.yml`, `.github/zizmor.yml`, `lychee.toml`, `.lighthouserc*.cjs`, `docs/` (23 files, Diataxis layout), 18 `scripts/`, `templates/` (32), `test/` (299). No `AGENTS.md`, no `CONTRIBUTING.md`; `CLAUDE.md` (11 lines) imports `.claude/rules.md`. No `hex-*` skills and no `.claude/hooks`, `.claude/tests`, `.claude/skills` directories were found in the tracked list.
- No reusable workflows or submodules; every workflow job uses `ocx-sh/setup-ocx` (pinned SHA, v1.3.0) to provision tools.
- How it consumes grim: `src/enrich/index.ts` is the one spawn site (`spawnGrim`, `execFile(bin, [...args, "--format", "json"])`; `grimVersion` runs `--version`), `src/grim.ts` holds `MINIMUM_GRIM_VERSION` and `MARKETPLACE_GRIM_VERSION`, `grim-indexer enrich` probes `grim --version` once and exits 69 below the floor (`.claude/rules/grim-compat-markers.md`). Schema/fixture coupling: `test/conformance/GRIM_REF` pins grimoire release tag `v0.15.0`; CI job `conformance` sparse-checks out `test/data/conformance` of `grimoire-rs/grimoire` at that tag and runs `npm run conformance:check` with `GRIM_CHECKOUT=.grim` (`ci.yml:~93-125`). A live job `live-grim` builds grim from the commit in `test/live/GRIM_COMMIT` (`84382946504fdd27dae166deb2b14ca2b53cbcf6`) with a local registry on `localhost:5000` (`ci.yml:127-170`). The indexer reserves the namespaces `grim`, `grimoire`, `index` (`src/cli/init.ts:800`).

### 3.3 Cross-repo summary

- Neither sibling uses a reusable workflow, a template repository, a submodule or a shared Taskfile include. Sharing is by copy, with drift, and (for `indexer` only) through `grimoire.toml` bundles from `ocx-sh/lore`.
- `grimoire.toml` in this repository names bundles from `michael-herwig/arcana`; `indexer` names bundles from `ocx-sh/lore`; the vendored-rule table in `.claude/rules.md` names the upstream `ocx-sh/grimoire-lore` and `michael-herwig/arcana`.
- Both siblings talk to grim only as a process with `--format json`, and both pin grim by version floor plus a commit or tag of grimoire for fixtures.

## 4. Existing statements

- `.claude/rules/subsystem-mirror.md:12`: "**No dependency on `grimoire`.** The crate must move to its own repository unchanged. `tests/boundary.rs` pins it". Line 13: "**Source-only, Unstable.** No release archive, no install path ... an Unstable sibling surface listed under `docs/src/content/docs/stability.md` until the split." Line 15: "**Git hardening is copied, not shared** from `src/catalog/git_cmd.rs` (no lib split)."
- `.agents/adr/adr_artifact_mirroring.md:82`: "**Chosen R2** ... matches ocx's own history (ocx-mirror was a workspace crate before `adr_cli_repo_split_template.md` split it). The crate talks to grim only through `grim` argv and `--format json` output; the one source file it shares textually is grim's exit-code table, read by a test (D14). The split is a follow-up issue for the owner: `git filter-repo --path crates/grim-mirror`, grim vendored as a submodule if the exit-code test is kept." Line 85 (shipping): "adding it to grim's 8-target release ties its cadence to grim's and creates an install path the split would break"; "Options not taken ... a separate `cargo-dist` app in this workspace (a second release pipeline to tear down at the split)." Line 59: "**Small frozen surface.** Anything a later split or a second consumer may need to change stays Unstable." Line 300 (revisit triggers): "The owner creates the mirror repository: run the split (follow-up issue), revisit D1 shipping." Line 307 (deferred): "Repository split; curated list repository and its job; `--announce` support in the mirror; ..." Line 280: `grim-mirror` surface is "Free until the split".
- `.agents/plans/plan_artifact_mirroring.md:45`: "File follow-up issues: repository split of `crates/grim-mirror` (ocx repo-split template); curated list repository ...". Line 52 (PD-1): "the owner ruled a separate repository; this run may change only this one ... Decision: **R2**, a workspace crate ... The split is follow-up 2." Line 102 repeats "no lib split exists and the crate must not depend on `grimoire`".
- `.agents/discussions/artifact-mirroring.md:115`: "Tool home, chosen by the owner: a separate repository and binary, plus grim gains PATH plugin dispatch so the tool runs as `grim mirror`, the shape `ocx mirror` has."
- `.claude/rules/product-context.md:106-110`: layout lists "the `crates/grim-mirror` sibling binary ... source-only, Unstable). No lib/CLI split of the main crate." Lines 117-128 "Related Repositories": `grimoire-vscode`, `grimoire-index` (served at `index.grimoire.rs`), `grimoire-components` ("Reusable CI/CD components for grim-based pipelines"), `arcana`, `external/rust-oci-client`, and `ocx-sh/index` as prior art. The table does not list `indexer` under that name and does not list `grim-mirror`.
- `AGENTS.md:19`: "no stable *library* API — the binary is the only consumer." `AGENTS.md:75-76` repeats the layout and "No lib/CLI split of the main crate." Principle 9 and the status banner declare a stabilization freeze for 1.0.0 with additive-only evolution.
- The word GUI, desktop application, Tauri or Electron does not appear in any of the five files (searched case-insensitively for `gui`, `desktop`, `tauri`, `electron`).

## Negative results

- No GUI, desktop or Tauri/Electron statement anywhere in the five named files or in the two ADR/discussion files.
- No `workspace.dependencies` table and no shared crate between grim and the mirror: the "no lib split" decision means shared logic is copied (`git.rs`) or pinned by `include_str!` tests (`exit_code.rs`, `git.rs`).
- Neither `vscode` nor `indexer` uses reusable workflows, a template repository or submodules; `vscode` has no `grimoire.toml` at all.
- `grimoire-components` ("reusable CI/CD components") is named in `product-context.md:123` but was not found as a checkout under `/home/mherwig/dev/grimoire-rs/` (only `vscode` and `indexer` were inspected as asked); whether it exists on GitHub was not checked.
- `dist-workspace.toml` and `deny.toml` do not name the mirror; no mirror-specific Taskfile task exists besides `test/taskfile.yml`.
- The mirror has no Windows build (`test/conftest.py:197-199`), while grim ships 8 targets including `aarch64`/`x86_64-pc-windows-msvc`.
- `indexer` has no `AGENTS.md`, `CONTRIBUTING.md`, hooks or `hex-*` skills; `vscode` has no `grimoire.lock`/`ocx.toml`.
- `.licenserc.toml` line count was not measured.

## Leads

- `grimoire-components` repository: the `product-context.md` table names reusable CI/CD components. Check whether it exists and what it exports, since it is the only declared sharing mechanism for pipelines.
- `ocx-sh/grimoire-lore` and `michael-herwig/arcana` upstream layout: these already distribute vendored rules and the `hex` bundle, so how bundles are cut there shows what a "base bundle" would look like.
- `adr_cli_repo_split_template.md` in ocx: the mirror ADR names it as the template for a satellite split; its steps (history filter, CI, release) are not in this repository.
- Measure diff between grimoire's `.github/workflows/verify-basic.yml` and the sibling `ci.yml` files: this recon compared only same-named files, so the real overlap of CI logic (setup-ocx, lint, release) is unknown.
- Check which `test/src/` harness modules the mirror would still need after a split (`runner.py`, `registry.py`, `zot.py`, `helpers.py`): 45 mirror tests depend on the shared `registry` and `grim_home` fixtures plus a built `grim`.
- Inspect `.claude/hooks` and `.claude/tests/test_ai_config.py` for hard-coded grimoire paths: they gate every commit here and a copy elsewhere would inherit those assumptions.
- GUI base: grim's JSON interface is documented in `docs/src/content/docs/json-interface.md` and `stability.md`; whether that contract is enough for a GUI (as it is for `vscode`) or a library API would be needed is not answered in any existing record.
