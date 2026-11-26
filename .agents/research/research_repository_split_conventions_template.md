# Research: ocx-sh/conventions copier template versus the grimoire scaffold

## Metadata

**Date:** 2026-11-25
**Domain:** ci-cd, packaging
**Triggered by:** repository-split discussion: two new grimoire-organisation repositories (grim-mirror plugin, desktop GUI) should carry "the same scaffold as grimoire"
**Expires:** 2027-02-25 (the template moves weekly; latest tag read was v0.3.6)

Sources read, all local and read-only: `/home/mherwig/dev/ocx-sh/conventions` (branch `fix/pin-from-recorded-commit`, tip `739ab84`, tag `v0.3.6`, 2 workflow templates and 4 generated copies carry uncommitted edits), `/home/mherwig/dev/ocx-sh/ocx-mirror` (main `0cb959a`), the grimoire worktree at `/home/mherwig/.herdr/worktrees/grimoire/uno`. File references below are relative to the conventions repo unless a path starts with a grimoire or ocx-mirror name.

## Direct Answer

The template is a tool, task, CI and hook baseline, not a language scaffold. It renders no Rust, Python or TypeScript configuration at all (no `Cargo.toml`, `rustfmt.toml`, `deny.toml`, `rust-toolchain.toml`, `pyproject.toml`, licence, contribution or code-of-conduct files, no `.claude/hooks`, no `.claude/settings.json`, no test harness, no docs site files, no release workflow). The `languages` answer only selects which `<lang>-essentials` grim bundles from `ghcr.io/ocx-sh/lore` go into `grimoire.toml`. Every tool pin comes from the ocx index (`ocx.sh/...`) and every CI job starts through `ocx-sh/setup-ocx`, so a repository adopting it must also adopt ocx as its toolchain manager. Grimoire does not: its `ocx.toml` uses floating pins and it has no `setup`, `lock:check` or `ci:*` tasks. The grimoire fleet row is `adopted = false`, with no recorded reason.

## 1. What the template provides

### Rendered files (MANIFEST.toml:12-153, `src/` tree)

Template-owned (rewritten on every update; line 1 is the banner `ocx-baseline: template-owned file`, JSON files carry a `description` key; `baseline:check` fails on a hand edit):

| Path | Profiles | What |
|---|---|---|
| `.copier-answers.yml` | full, mirror | answers and `_commit` |
| `taskfiles/baseline.taskfile.yml` | both | tasks `setup`, `hooks:install`, `hooks:commit-msg`, `hooks:repo-pre-commit`, `lock:check`, `grim:check`, `secrets`, `workflows:lint`, `baseline:check`, `release:prepare` (plus internal `release:run`, `release:apply`), `copier:update`; pins `COPIER_VERSION` 9.18.2 and `ZIZMOR_VERSION` 1.30.1 at lines 9-11 |
| `.pre-commit-config.yaml` | both | prek hooks: gitleaks on staged files, conventional-commit subject check (types feat fix docs style refactor perf test build ci chore revert), optional repo `hooks:pre-commit` |
| `cliff.toml` | both | git-cliff keepachangelog config, `chore`, `ci`, `style`, `test`, `build` skipped, `initial_tag = "v0.1.0"`, pre-1.0 bump rules (`src/cliff.toml.jinja:68-70`) |
| `renovate.json` | both | extends `github>ocx-sh/conventions//renovate/default#<tag-or-sha>` then `local>{repo_owner}/{repo_name}//.github/renovate-local` |
| `CLAUDE.md` | both | the single line `@AGENTS.md` |
| `.github/workflows/verify-basic.yml` | both | jobs lint, test, build (bazel), docs (site), repo (repo_workflows), `basic-gate` (the one required check); mirror profile gets lint and gate only |
| `.github/workflows/verify-deep.yml` | full | nightly plus `run-deep` label plus `workflow_call`, matrix over `deep_os`, opens or closes a `nightly-red` issue |
| `.github/workflows/claude.yml` | both | PR review plus `@claude` responder plus issue handler; secret `CLAUDE_CODE_OAUTH_TOKEN`, skips when empty or fork |
| `.github/workflows/issues.yml` | both | Discord ping for outsider issues, org secret `OCX_ISSUES_HOOK` |
| `.github/workflows/deploy-site.yml` | full, only when `site.deploy` | Bunny deploy through `ocx-sh/website/.github/actions/deploy`, environment `ocx.sh`, secret `BUNNY_STORAGE_KEY` |
| `.github/zizmor.yml` | both | hash-pin policy, `zizmor_ignore` entries |
| `.github/ISSUE_TEMPLATE/config.yml`, `report.yml` | both | blank issues off, one report form assigned to `maintainers` |
| `bazel/baseline.bazelrc` | full, only when `bazel` | remote cache `bazel-cache.ocx.sh`, `test:basic` and `test:deep` configs |
| `.claude/skills/.gitattributes`, `.claude/rules/.gitattributes`, `.opencode/skills/.gitattributes`, `.opencode/rules/.gitattributes`, `.agents/skills/.gitattributes` | both | mark grim-installed client files `linguist-generated` |

Seeded once (`copier.yml:10-19` `_skip_if_exists`; the adopter owns them afterwards):

| Path | Seed content |
|---|---|
| `taskfile.yml` | includes the baseline taskfile flat; stub `verify`, `ci:lint`, `ci:test`, `ci:build`, `ci:docs`, `ci:deep` |
| `ocx.toml` | `[tools] task`; `[group.lint]` prek, gitleaks, actionlint, grim 0.14.3, uv; empty `[group.test]`; `[group.build]` bazelisk if bazel; `[group.docs]` node and pnpm if site; `[group.release]` git-cliff (`src/ocx.toml.jinja`) |
| `grimoire.toml` | `clients = ["claude","codex","opencode"]`, bundles `docs-essentials`, `code-docs-essentials`, one `<lang>-essentials` per language, `ocx-theme` if `theme_bundle`, all `ghcr.io/ocx-sh/lore/...:latest` (`src/grimoire.toml.jinja`) |
| `AGENTS.md` | TODO skeleton (Purpose, Setup, Verify, Layout, Baseline) |
| `.gitleaks.toml` | `useDefault` plus a line-regex allowlist for the ocx-mirror Discord id |
| `.github/renovate-local.json` | description only |
| `.github/workflows/basic-repo.yml`, `deep-repo.yml` | `workflow_call` stubs, only when `repo_workflows` |
| `.bazelrc`, `MODULE.bazel` | only when `bazel` |

On adoption `adopt.sh` removes `.github/dependabot.yml` and `Taskfile.yml` (`MANIFEST.toml:7-8`, `scripts/adopt.sh:68-78`), renames `Taskfile.yml` to `taskfile.yml`, and moves non-pointer content of `CLAUDE.md` into `AGENTS.md`. Grim client output (`.claude/skills`, `.claude/rules`, `.claude/settings.json`, `opencode.json`, ...) is committed by the adopter, not rendered by copier (`grim:check`, `src/taskfiles/baseline.taskfile.yml:81-103`).

### Copier questions (`copier.yml`, `docs/contract.md:9-24`)

| Question | Type, choices | Default | Asked when |
|---|---|---|---|
| `repo_name` | str | none | always |
| `repo_owner` | str | `ocx-sh` | always |
| `profile` | `full`, `mirror` | `full` | always |
| `maintainers` | yaml list of GitHub logins | `["michael-herwig"]` | always |
| `bazel` | bool | false | full |
| `languages` | multiselect: bazel cmake dotnet go jvm nix python rust shell swift typescript | `[]` | full |
| `site` | null or `{dir, dist, deploy, path?, paths?}` | null | full |
| `deep_os` | multiselect: ubuntu-latest ubuntu-24.04-arm macos-latest macos-15-intel macos-26 windows-latest windows-11-arm | `[ubuntu-latest]` | full |
| `bazel_cache_write` | `main`, `trusted` | `main` | full and bazel |
| `submodules` | bool | false | full |
| `repo_workflows` | false or `{secrets: [NAME,...]}` | false | full |
| `zizmor_ignore` | list of `.github/workflows/<name>.yml` | `[]` | full |
| `theme_bundle` | bool | `site is not none` | full |

Minimum copier is 9.18.2. The template uses no `_tasks`, `_migrations` or `_jinja_extensions`, so `--trust` is never needed (`copier.yml:1-3`).

### Kinds and languages

Profiles: `full` (repository with product code) and `mirror` (ocx-contrib `mirror-*` repositories: lint and gate only). Language support is only the bundle list above; `rust`, `python`, `typescript`, `shell`, `bazel` and others have no rendered config. Test, build and docs behaviour is whatever the adopter's own `ci:*` tasks do. A site is assumed Astro, mkdocs or similar and only drives the `docs` job, the `node`/`pnpm` group and the optional Bunny deploy.

### Hard-wired to ocx (what a grimoire-organisation repository cannot use as is)

- Tool management: `ocx.toml` and `ocx.lock` are mandatory; `baseline:check` requires exact `X.Y.Z` refs and the baseline tool set (`scripts/baseline_check.py:25-27`, `docs/contract.md:100-104`). Grimoire's own `ocx.toml` uses floating refs (`zot:2`, `node:24`, `git-cliff:latest`), which the check refuses.
- Every task body calls `ocx exec`; every workflow job starts with `ocx-sh/conventions/.github/actions/setup@<sha>`, which installs ocx through `ocx-sh/setup-ocx` (`src/.github/workflows/verify-basic.yml.jinja:1-3`).
- Tool pins resolve against the `ocx.sh` index (`scripts/groups.toml` repository map), including `ocx.sh/grimoire/cli` for grim.
- Org-level secrets and hosts: `BAZEL_CACHE_*`, `OTEL_OTLP_AUTH` (`otel.ocx.sh`), `OCX_ISSUES_HOOK`, `CLAUDE_CODE_OAUTH_TOKEN`, `BUNNY_STORAGE_KEY`, bazel cache `bazel-cache.ocx.sh`. Missing secrets degrade to skip, they do not fail.
- Hard-coded owner `ocx-sh`: the `cliff.toml` footer links use `https://github.com/ocx-sh/{{ repo_name }}` (`src/cliff.toml.jinja:30-48`), ignoring `repo_owner`; zizmor policy lists `ocx-sh/*: hash-pin` (`src/.github/zizmor.yml.jinja`); setup action and Renovate preset live in `ocx-sh/conventions`; the `deploy-site` action is `ocx-sh/website`.
- `grimoire.toml` bundles come from `ghcr.io/ocx-sh/lore` (`ocx-sh/grimoire-lore`), and `grim:check` requires clients claude, codex and opencode (grimoire itself sets `clients = ["claude"]`, `grimoire.toml:4`).
- `.gitleaks.toml` and `workflows:lint` carry ocx-mirror specifics (Discord id regex, "Generated by ocx-mirror" skip).
- Bazel is optional (`bazel` answer), not required. There is no ocx-specific release step in the template: `release:prepare` is generic git-cliff plus a green-nightly gate, and never tags or pushes. The repo-wide release machinery (tagging, publishing) is repo-owned.
- The conventions repository publishes a grim rule and skill `ocx-baseline` (`publish.toml`, `.github/workflows/publish.yml`) that guide adoption.

## 2. How updates reach adopters

- Pull flow: Renovate's copier manager runs `copier update` against each adopted repository and opens a PR; the preset auto-merges conventions updates once the required `basic-gate` is green, ignoring the 7-day cooldown and weekly window (`renovate/default.json:160-168`). Renovate runs self-hosted from `.github/workflows/renovate.yml` (hourly on weekdays, one scoped GitHub App token per owner, repository list taken from every `adopted = true` and not excluded row in `fleet.toml`); it needs `RENOVATE_APP_ID` and `RENOVATE_APP_KEY` in environment `renovate`. Hosted Renovate must be removed from an adopted repository to avoid doubled PRs (`docs/contract.md`, last paragraph).
- Manual flow: `ocx exec -- task copier:update -- --vcs-ref <tag>` (`copier update --defaults`, pinned copier). Adoption of an existing repository: `scripts/adopt.sh <dest> <fleet-key>` (stages, never commits, needs a clean tree, reads answers from the fleet row, defaults to the newest `v*` tag); a new repository: `copier copy gh:ocx-sh/conventions <dir>` (README.md "Adopt").
- Versions: git tags `vX.Y.Z` on conventions (latest `v0.3.6`, commit `739ab84`, 2026-11-17); the template workflows pin the setup action by commit (`vcs_ref_hash`) and the Renovate preset by tag or commit, so both move only through copier. A tag push runs `publish.yml`, which publishes the `ocx-baseline` rule and skill to `ghcr.io/ocx-sh/conventions` with `grim publish`.
- Drift checker: `drift/check.py` reads `fleet.toml` plus remote facts through `gh api` and writes `status.json`; `drift/render.py` renders the HTML page; `.github/workflows/status.yml` runs daily (05:17 UTC) and publishes to GitHub Pages. Flags: `baseline-behind`, `stale-pin`, `setup-ocx-stale`, `ocx-cli-stale`, `pending-updates` (open Renovate PRs), `nightly-red`, `bundle-missing`, `grim-pending` (never raised online, `drift/check.py` ponytail comment), plus a `not-adopted` state (`drift/check.py:206-247`, `tests/test_drift.py:21`).
- `task baseline:check` (`src/taskfiles/baseline.taskfile.yml:105-130`, `scripts/baseline_check.py`): clones the template at the recorded `_commit`, re-renders with the recorded answers into a temp dir and diffs the template-owned paths; then checks that root tasks `verify`, `ci:lint`, `ci:test`, `ci:build`, `ci:docs`, `ci:deep` exist, `ocx.toml` has exact pins and the baseline tools in the right groups, `grimoire.toml` has the clients and the expected bundles, Bazel rules (`rules_ocx` exact dep, no `http_archive` for a mapped tool, `.bazelrc` ends with the user-rc import), a Lighthouse gate on `ci:docs` when a site exists, and the pnpm pin. It runs in `verify-basic` job `lint`.
- Local divergence: a hand edit to a template-owned file fails `baseline:check` (red `lint` job, so red `basic-gate`) and would be overwritten by the next `copier update`. Seeded-once files are never touched again. A behind adopter is only flagged on the status page, not blocked. Divergence an adopter needs in a template-owned file has no escape hatch except changing the template (documented rule: "change ocx-sh/conventions, then run task copier:update"); the sanctioned extension points are the repo-owned hooks `hooks:pre-commit`, `hooks:repo-commit-msg`, `release:bump-files`, `release:gate`, `release:validate`, `release:verify`, the `repo_workflows` stubs, `.github/renovate-local.json`, `zizmor_ignore` and `.gitleaks.toml`.

## 3. Fleet state for grimoire

`fleet.toml` header: "every repo the OCX baseline tracks. Read by scripts/adopt.sh, drift/check.py and the Renovate runner". Every grimoire-organisation row (lines 104-181, 315-330, 399-405):

| Key | `repo` | iteration | adopted | excluded | Answers |
|---|---|---|---|---|---|
| `grimoire` | `grimoire-rs/grimoire` | 2 | false | false | profile full, bazel false, site `{dir="docs", dist="docs/dist", deploy=false}` (comment: "Astro (docs/astro.config.mjs); Pages, no deploy"), languages `["rust","typescript"]`, deep_os ubuntu, macos, windows, theme_bundle false, submodules true |
| `grimoire-index` | `grimoire-rs/index` | 2 | false | false | full, languages `["typescript"]`, deep_os ubuntu |
| `grimoire-indexer` | `grimoire-rs/indexer` | 2 | false | false | full, site `{dir=".", dist="site", deploy=false}` (mkdocs), languages `["typescript","python"]` |
| `grimoire-lore` | `ocx-sh/grimoire-lore` ("origin is ocx-sh, not grimoire-rs") | 2 | false | false | full, site docs (comment "TODO verify in iteration 2"), languages `["python"]` |
| `grimoire-vscode` | `grimoire-rs/grimoire-vscode` | 2 | false | false | full, languages `["typescript"]`, deep_os ubuntu, macos, windows |
| `setup-grimoire` | `grimoire-rs/setup-grimoire` | 2 | false | false | full, site `{dir="site", dist="site", deploy=false}` (static HTML), languages `["shell"]`, deep_os all three |
| `grimoire-components` | `grimoire-rs/components` (gitlab.com) | none | false | true | `reason = "GitLab-hosted, outside the GitHub fleet (D-10)"` |

The only recorded reason for any non-adoption is the `excluded` reason on `grimoire-components`. Iteration 2 is the planned fleet wave ("1 canary, 2 fleet, 3 mirrors", `fleet.toml:9`); iteration 2 rows are all `adopted = false` except in the grimoire family none is adopted. Adopted rows are `conventions` (1), `ocx-website`, `devcontainer-features`, `ocx`, and the ocx-contrib mirrors. There is no row for a `grim-mirror` repository or a desktop GUI repository.

Nothing else in conventions records why grimoire is not adopted or what blocks it: `.agents/` holds only `.gitattributes`-style placeholders (no ADRs, notes or discussions), `docs/` holds only `contract.md`, the changelog and README do not mention grimoire adoption, and the commit log has no grimoire-specific entry besides submodule support (`18e0c5c`, `f1963b3`: the `submodules` answer was added for ocx, grimoire and ocx-mirror) and `repo_workflows` / trusted Bazel cache (`9a68818`). `skills/ocx-baseline/references/best-in-fleet.md:12` names ocx and grimoire as best-in-fleet examples for Rust build, lint, deny, taplo, hawkeye, nextest JUnit and cargo-dist. Issues in the GitHub repositories were not read (no network, GitHub connector offline).

Inconsistency noticed: `fleet.toml` lists `ocx-mirror` as `adopted = false` (line 286), yet `ocx-mirror/.copier-answers.yml` records `_commit: v0.3.5` and its history has `chore(baseline): conventions v0.3.5` (`2a7b40a`). The fleet file is behind reality for that repository, and the Renovate runner, which only serves `adopted = true` rows, would skip it.

## 4. Comparison with grimoire's scaffold

Grimoire facts: `.github/workflows/` has `acceptance.yml`, `backfill.yml`, `claude.yml`, `docs.yml`, `publish-catalog.yml`, `release-ocx.yml`, `release.yml` (generated by cargo-dist), `verify-basic.yml`, `verify-deep.yml`; composite actions `.github/actions/build-rust` and `release-ocx`; `.github/dependabot.yml` exists (the template removes Dependabot); `CODEOWNERS`, ISSUE_TEMPLATE with `bug_report.yml`, `config.yml`, `feature_request.yml`.

| Grimoire piece | Template | Notes |
|---|---|---|
| `verify-basic.yml` (238 lines: conventional commits via cocogitto, DCO sign-off, supply-chain, build and test jobs) | diverging equivalent | template job set is lint, test, build, docs, repo, `basic-gate`, driven by `ci:*` tasks and ocx setup; no DCO job, no cocogitto, no cargo-deny, no Rust cache or build action |
| `verify-deep.yml` (634 lines: build and unit test matrix, cross-compile, hook size on macOS, Windows hook tests, Windows platform check, mod-load probe, test-results) | diverging equivalent | template deep is one matrix job calling `task ci:deep` plus the `nightly-red` issue; grimoire's content would live in `deep-repo.yml` or `ci:deep` |
| `acceptance.yml`, `backfill.yml`, `docs.yml`, `publish-catalog.yml`, `release-ocx.yml`, `release.yml`, `.github/actions/*` | nothing | repo-owned in ocx-mirror too (`deploy-site.yml` is the only site workflow, Bunny-specific) |
| `claude.yml` (44 lines) | diverging equivalent | template version is longer: PR review, `@claude` responder, issue handler with hidden-content pre-check and `claude/` branch commits, ruleset advice |
| `dependabot.yml` | opposite | removed on adoption, replaced by self-hosted Renovate |
| `.github/ISSUE_TEMPLATE/*`, `CODEOWNERS` | partial | template has one `report.yml`; grimoire has bug and feature forms |
| `taskfile.yml`, `taskfiles/*.taskfile.yml` (bench, coverage, docs, git, p9, release, rust, schema, shell, upstream) | diverging equivalent | template has only `baseline.taskfile.yml` plus a stub root taskfile with `ci:*` names; grimoire has no `ci:*`, `setup` or `lock:check`, its two-phase `verify` (`.verify:lint`, `.verify:build-test`, `.verify:mark`) and `checkpoint`, `push`, `.ensure-cargo-tool` are repo-specific; template `release:prepare` versus grimoire's `taskfiles/release.taskfile.yml` (70 lines) and `release:` ceremony |
| `.claude/hooks/*` (10 Python hooks incl. `pre_commit_verification.py`, the commit guard requiring a `commit-verified` stamp under `.claude/hooks/.state` no older than 5 minutes, written by `task verify`'s `.verify:mark`) | nothing | the template installs prek git hooks (gitleaks, conventional subject, optional task hook), not Claude Code hooks; the guard and `pre_push_main_blocker.py` have no equivalent |
| `.claude/settings.json` (hook wiring) | nothing | the template ships none; `grim` writes `.claude/settings.json` for installed hook kinds and `grim:check` fails when it differs from the install |
| `.claude/tests/test_ai_config.py` (53 tests, catalog and rule-glob structure) plus `test_hooks.py`, `test_workflows.py`, `test_upstream_stale.py` | nothing | the template has no structural test for AI config; its own `tests/test_ai_config.py` tests the conventions repo, not adopters |
| `.claude/templates/` (artifact and mechanism templates) | nothing | |
| `.claude/rules/` and `rules.md` catalog (first-party `subsystem-*`, `quality-*`, vendored docs, rust, python, typescript, code-docs rules) | partial | the template ships only `.gitattributes` for client dirs; rules arrive through lore bundles in `grimoire.toml` (`rust-essentials`, `python-essentials`, `typescript-essentials`, docs bundles), which is how grimoire's vendored rules were originally sourced (`.claude/rules.md` "Vendored rules" names `ocx-sh/grimoire-lore`) |
| `grimoire.toml` / `grimoire.lock` (clients `["claude"]`; bundles `hex`, `nox` from `ghcr.io/michael-herwig/arcana`) | diverging equivalent | template requires clients claude, codex, opencode and the lore bundles; grimoire has none of the lore bundles and its two bundles are not in the template |
| `ocx.toml` / `ocx.lock` | diverging equivalent | both exist; grimoire pins loosely (`zot:2`, `node:24`, `cargo-deny:0.20`, `hawkeye:7`, `lychee:0.24`, `ruff:0.17`, `shellcheck:0.11`, `shfmt:3`, `uv:0.13`, `git-cliff:latest`), single `[tools]` table plus group `g0`, no `task` pin, no lint/test/build/docs/release groups, grim 0.14.3 only for the index test; template requires exact pins and groups |
| `AGENTS.md` / `CLAUDE.md` | diverging equivalent | both use the AGENTS.md-canonical plus `@AGENTS.md` pointer shape; template `CLAUDE.md` is the bare line `@AGENTS.md` with a banner and is template-owned, grimoire's `CLAUDE.md` carries explanatory prose and would be overwritten; grimoire's `AGENTS.md` is a long hand-written file, the template seeds a TODO skeleton |
| `deny.toml` | nothing | cargo-deny config repo-owned (`deny.toml:1-40`: licence allow-list, advisories policy, bans, sources); `ocx.toml` carries `cargo-deny` |
| `rustfmt.toml`, `rust-toolchain.toml` (channel 1.95.0), `clippy.toml`, `taplo.toml` | nothing | |
| `cliff.toml` | diverging equivalent | both git-cliff keepachangelog; template one is owned and hard-codes `ocx-sh` footer URLs and skips `chore ci style test build`; grimoire's is "customized for Grimoire" and grimoire additionally has `cog.toml` for cocogitto |
| `dist-workspace.toml` (cargo-dist 0.33.0, 8 targets, shell and powershell installers, post-announce jobs) | nothing | the generated `release.yml` would need `zizmor_ignore = [".github/workflows/release.yml"]`, the documented use of that answer |
| `.licenserc.toml` (hawkeye SPDX header, `src`, `crates`, `build.rs`, `build/`) | nothing | `hawkeye` is in the template's tool map but not in any seeded `ocx.toml` and has no config |
| DCO sign-off check (`taskfiles/git.taskfile.yml` `dco`, job `dco` in `verify-basic.yml`) | nothing | template commit-msg hook checks only the conventional subject, and `hooks:repo-commit-msg` is the replacement point; no sign-off check |
| pytest acceptance harness (`test/conftest.py`, `test/src/{assertions,consent_driver,git_fixture,helpers,hook_launcher,index_fixture,oci_proxy,old_grim,otlp_fixture,registry,runner,scenarios,stdio_drivers,zot}.py`, `test/taskfile.yml` with `GRIM_BINARY`, `GRIM_MIRROR_BINARY`, `ocx exec -- uv run pytest`, zot registry started by conftest) | nothing | only conventions' own `tests/` (pytest self-tests); template gives `uv` in group `lint`, and in group `test` when `python` is a language, and requires `ci:test` and `ci:deep` to write JUnit under `target/reports/junit/` (the harness already writes reports under `target/reports`, `REPORTS_ROOT` in `taskfile.yml:6`) |
| docs site under `docs/` (Astro Starlight, `package.json`, `astro.config.mjs`, `docs.toml`, landing and URL checks) | nothing | only the `site` answer: a `docs` job, node and pnpm in group `docs`, a Lighthouse requirement on `ci:docs`, optional Bunny deploy; the fleet row for grimoire sets `deploy = false` |
| `.gitleaks`, secret scanning | template only | grimoire has no gitleaks task; template runs `task secrets` and a pre-commit gitleaks hook |
| `.pre-commit-config.yaml` / prek | template only | grimoire has Claude Code hooks instead of git hooks |
| `CODE_OF_CONDUCT.md`, `CONTRIBUTING.md`, `SECURITY.md`, `LICENSE`, `CHANGELOG.md` | nothing (CHANGELOG written by `release:prepare`) | |
| git submodules (`external/docker_credential`, `external/rust-oci-client`) | supported | answer `submodules = true` adds `submodules: recursive` to every checkout; the grimoire fleet row sets it |

## 5. An adopter in practice: ocx-mirror

Answers (`ocx-mirror/.copier-answers.yml`, `_commit: v0.3.5`, `_src_path` the public conventions URL): profile full, `repo_owner` ocx-sh, `bazel` true, `bazel_cache_write` trusted, `languages` bazel, python, rust, shell, `deep_os` `[ubuntu-latest]`, `maintainers` `[michael-herwig]`, `repo_workflows` secrets `BAZEL_CACHE_WRITE_AUTH`, `BAZEL_CACHE_READ_AUTH`, `OTEL_OTLP_AUTH`, `site` `{deploy: false, dir: ".", dist: "site"}` (mkdocs), `submodules` true, `theme_bundle` false, `zizmor_ignore` `[]`. History shows adoption steps `ci: adopt conventions v0.3.2`, `v0.3.4`, `chore(baseline): conventions v0.3.5` and a later bump of the setup pins to setup-ocx v1.4.0 and ocx 0.6.6 (`0e52525`).

Template-owned files present (banner found): `.github/workflows/verify-basic.yml`, `verify-deep.yml`, `claude.yml`, `issues.yml`, `.github/zizmor.yml`, `.github/ISSUE_TEMPLATE/config.yml`, `report.yml`, `.pre-commit-config.yaml`, `cliff.toml`, `renovate.json`, `CLAUDE.md`, `taskfiles/baseline.taskfile.yml`, `bazel/baseline.bazelrc`, and the five `.gitattributes` under `.claude/skills`, `.claude/rules`, `.opencode/skills`, `.opencode/rules`, `.agents/skills`.

Seeded-once files it now owns: `taskfile.yml` (includes the baseline plus `rust`, `release`, `docs`, `git`, `bazel`, `telemetry`, `claude`, `test` taskfiles), `ocx.toml` (exact pins, groups lint, test, build, docs, release, extra tools `zot`, `lychee`, `bazel`, `cargo-cyclonedx`, and a release group `ocx` pin differing on purpose), `grimoire.toml` (the six lore bundles bazel, code-docs, docs, python, rust, shell), `AGENTS.md` (long hand-written), `.gitleaks.toml`, `.github/renovate-local.json` (submodule and pip-compile rules), `.github/workflows/basic-repo.yml` and `deep-repo.yml` (the repo's real jobs), `.bazelrc`, `MODULE.bazel`.

Scaffold still carried by hand outside the template: workflows `acceptance.yml`, `backfill.yml`, `build-matrix.yml`, `canary.yml`, `docs.yml`, `prune.yml`, `readiness-gate.yml`, `release-readiness.yml`, `release.yml`, `verify-version.yml`; composite actions `bazel-cache-rc`, `release-ocx`, `test-telemetry`; `.claude/` hooks (`pre_tool_use_guard.py` and its test only), `.claude/settings.json`, `.claude/agents`, `.claude/templates`, a `.claude/taskfile.yml`, about 20 skills and the vendored rule set (`rust-quality`, `python-quality`, `shell-quality`, `bazel-quality`, `docs-quality`, `code-docs`, powershell rules, plus first-party `subsystem-mirror.md`, `quality-*.md`, `meta-*.md`); `deny.toml`, `rustfmt.toml`, `rust-toolchain.toml`, `taplo.toml`, `.licenserc.toml`, `.taskrc.yml`, `.config/`, `Cargo.toml` workspace, `release/` and `packaging/` directories, the `test/` and `tests/` trees, `mkdocs.yml` and `docs/`, `LICENSE`, `CATALOG.md`, `.gitmodules` with `external/ocx`. It has no `CONTRIBUTING.md` or `SECURITY.md` in its tree. So the template covers CI skeleton, toolchain, hooks, changelog config, Renovate and the agent pointer; everything language-, release- and test-specific stays repo-owned.

## Negative results

- No ocx-specific release step in the template, but also no release workflow, no cargo-dist, no tag automation.
- No DCO check, no cocogitto, no cargo-deny, no licence-header (hawkeye) config, no rustfmt or clippy configuration, no toolchain pin file.
- No Claude Code hook, commit-guard or `.claude/settings.json` equivalent; no AI-config structural test.
- No pytest acceptance harness, registry fixture or `test/` scaffold; no docs site scaffold files.
- No notes, ADR or discussion in conventions explaining why grimoire is unadopted; `.agents/` and `docs/` there are empty of such records.
- No fleet row for `grim-mirror` or a desktop GUI repository; no mention of Tauri or desktop apps.
- GitHub issues and PRs of conventions and grimoire were not checked (offline).
- Template carries no `CONTRIBUTING.md`, `SECURITY.md`, `CODE_OF_CONDUCT.md`.

## Leads

- `/home/mherwig/dev/ocx-sh/conventions/docs/contract.md`: the complete adopter contract (answers, tasks, workflows, hooks), the file to read before deciding on adoption.
- `/home/mherwig/dev/ocx-sh/conventions/scripts/baseline_check.py:25-27,126-170`: where exact-pin and bundle requirements are enforced, i.e. what grimoire's `ocx.toml` and `grimoire.toml` would fail today.
- `/home/mherwig/dev/ocx-sh/conventions/src/cliff.toml.jinja:30-48`: hard-coded `ocx-sh` owner in a template-owned file, the first thing that breaks for a grimoire-rs repository (`repo_owner` is not used there).
- `/home/mherwig/dev/ocx-sh/ocx-mirror/.github/workflows/basic-repo.yml` and `deep-repo.yml`: the working pattern for putting repo-specific jobs behind the template's `repo_workflows` hook.
- `/home/mherwig/dev/ocx-sh/conventions/fleet.toml:286`: `ocx-mirror` still `adopted = false` although adopted at v0.3.5, so the Renovate runner skips it.
