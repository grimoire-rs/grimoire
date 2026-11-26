# Research: How ocx split ocx-mirror out, and what it still has open

## Metadata

**Date:** 2026-11-25
**Domain:** ci-cd, packaging
**Triggered by:** Discussion on moving `grim-mirror` and a GUI app into their own repositories with the same scaffold as the core repository (`.agents/discussions/repository-split.md`).
**Expires:** 2027-02-25 (the ocx repositories move weekly; re-verify the commit and version numbers first)

Method: read-only. Plain file reads, `git log`, `git show`, hash comparison of tracked files. Nothing was fetched, built or run. Paths are under `/home/mherwig/dev/ocx-sh/` unless stated. Checkout state at reading time: `ocx` on `main` (1412 commits), `ocx-mirror` on `main` (418 commits), `conventions` on `main`.

## Direct Answer

Facts only, no recommendation.

1. ocx split its mirror tool into `ocx-sh/ocx-mirror` on 2026-06-12 (commit `1476192c` in `ocx`). The history was moved with `git filter-repo`. The two repositories link by a git submodule (`external/ocx` inside `ocx-mirror`) and cargo path dependencies into it, and they also talk through the `ocx` binary (spawned as a child process).
2. ocx later built a separate repository, `ocx-sh/conventions`, that renders the shared scaffold into every repository with `copier` and tracks drift in a `fleet.toml`. `ocx` and `ocx-mirror` both consume it, but at different versions (`v0.3.6` and `v0.3.5`), and both still carry large repository-owned files that differ.
3. The ADR written at split time named two follow-ups as unchecked: extract a template at the second satellite, and ship shared AI rules through grimoire. Both were later done in another form (copier template, `grimoire.toml` bundles).
4. No statement about a GUI or desktop application was found in any of the three repositories.

## 1. Repository layout of the ocx project

Directories under `/home/mherwig/dev/ocx-sh/` (each a git checkout): `catalog`, `conventions`, `devcontainer-features`, `e2e-index`, `grimoire-lore`, `index`, `indexbot`, `ocx`, `ocx-mirror`, `rules_ocx`, `website`, `www-setup`. Worktrees: `/home/mherwig/.herdr/worktrees/ocx/{duo,qui,quo,tri,uno}` and `/home/mherwig/.herdr/worktrees/ocx-mirror/uno`.

Purpose of the ones that matter here:

| Repository | Purpose | Evidence |
|---|---|---|
| `ocx` | The core CLI and the 25-member cargo workspace | `ocx/Cargo.toml:1-10` (`members = ["crates/*"]`), `ocx/AGENTS.md:77` ("25 workspace members") |
| `ocx-mirror` | Mirror tool, own binary `ocx-mirror`, own release cadence | `ocx-mirror/AGENTS.md:3-9` |
| `conventions` | Copier template, shared Renovate presets, composite setup action, drift checker over `fleet.toml` | `conventions/README.md:3-5` |
| `grimoire-lore` | Upstream of the vendored rule bundles (`rust-essentials` and others) | `ocx-mirror/grimoire.toml` pulls `ghcr.io/ocx-sh/lore/*-essentials` |
| `rules_ocx` | Bazel rules | `conventions/fleet.toml:20-33` (entry, iteration 1) |

`ocx` structure: workspace version `0.6.8`, Rust edition 2024, `resolver = "3"` (`ocx/Cargo.toml:10-17`). Crate directories under `ocx/crates/`: `ocx_announce`, `ocx_cli`, `ocx_config`, `ocx_console`, `ocx_env`, `ocx_exit`, `ocx_exit_derive`, `ocx_index`, `ocx_oci`, `ocx_package`, `ocx_package_manager`, `ocx_project`, `ocx_python`, `ocx_schema`, `ocx_script`, `ocx_sdkgen`, `ocx_setup`, `ocx_shell`, `ocx_shim`, `ocx_sign`, `ocx_store`, `ocx_telemetry`, `ocx_test_support`, `ocx_trust`, `ocx_util`.

There is a library versus CLI split, and it has stability tiers (`ocx/AGENTS.md:13-17`):

- Internal crates "have no stability at all". Quote: "No `ocx_*` crate is a published library; the binary is the only consumer." (`ocx/AGENTS.md:15`).
- Ten crates are tier "ecosystem", defined as "a crate a lockstep submodule consumer links": `ocx_util`, `ocx_env`, `ocx_console`, `ocx_oci`, `ocx_trust`, `ocx_sign`, `ocx_config`, `ocx_index`, `ocx_package`, `ocx_python` (`ocx/AGENTS.md:17`). Breaking changes are allowed, but the consumer must be upgraded "in the same change series", enforced by `task satellite:verify` (a job in `deep-repo.yml`).
- `ocx_cli` is tier "interface" (the application layer), and `ocx_shim` and `ocx_exit` are interface too (`ocx/AGENTS.md:104`, `ocx-mirror/Cargo.toml:41-43`).
- No crate is on crates.io. `ocx-sdk` and `ocx.sh/ocx/sdkgen` are planned published outputs (see section 4).

## 2. The `ocx-mirror` split

### What happened

- The mirror began inside `ocx` as a prototype: `a365c3a5` 2026-03-11 "feat: add ocx-mirror prototype for mirroring GitHub releases to OCI registries" (in `ocx`); the same commit is the root commit of `ocx-mirror` (`942b8fa`, 2026-03-11), so history was kept.
- Split commit in `ocx`: `1476192c` 2026-06-12 "refactor!: move ocx-mirror to its own repository". Quoted from its message:
  - "full history extracted via git filter-repo (crate, acceptance tests, mirror.yml docs, design records)."
  - "The satellite repo vendors ocx as a git submodule and consumes ocx_lib as a path dependency (fork-style, no published crate)".
  - "ocx side: workspace member removed, five mirror-only workspace deps pruned (octocrab, quick-junit, url, reqwest, rustls), CI drops the mirror build/upload/publish legs".
  - "Pattern recorded in adr_cli_repo_split_template.md for future satellite CLIs (ocx-mcp, ocx-dist)."
  - It carries a `BREAKING CHANGE:` footer.
- First scaffold commits in `ocx-mirror` the same day: `903eea5` 2026-06-12 "chore: scaffold standalone repository", `c08b577` "ci: add verify, build-matrix, oci-publish, deploy-dev, rel...", `e2e10dc` "chore(claude): port architect, swarm, commit, finalize ski...".
- ADR: `ocx/.claude/artifacts/adr_cli_repo_split_template.md`, Status Accepted, 2026-06-12, owner Michael Herwig. Quote of the decision: "Option 1, 'easy-to-maintain dependency rather than a published crate' (fork-style vendoring, reversed: satellite vendors ocx)." Rejected: Option 2 (ocx mounts the satellite as a workspace member, "Cross-repo dance on every breaking lib change") and Option 3 (publish `ocx_lib` to crates.io, "Pre-1.0 lib API churn = constant release ceremony"). The ADR's contract has eight numbered points: crate shape, dependency wiring, bump procedure, CI shape (five workflows), registry layout, versioning, repo scaffold, history. Point 7 reads: "copies of rustfmt/deny/.licenserc/cliff/taskfile-verify contract; `ocx.toml` + `.envrc` direnv toolchain; `.claude/` subset (quality rules copied; subsystem rule moves with the code, paths rescoped to `src/**`)".
- Related earlier ADR (Proposed, 2026-05-28): `ocx/.claude/artifacts/adr_cli_plugin_pattern.md`. Its implementation: commit `065895a0` 2026-05-28 "feat(cli): dispatch unknown subcommands to ocx-<name> plu...".

### How `ocx-mirror` depends on ocx today

Three mechanisms at once:

1. **Git submodule plus path dependencies.** `ocx-mirror/Cargo.toml` (workspace dependencies, lines 29-60 region) lists ten `ocx_*` path rows into `external/ocx/crates/...` (`ocx_config`, `ocx_console`, `ocx_env`, `ocx_exit`, `ocx_index`, `ocx_oci`, `ocx_package`, `ocx_sign`, `ocx_util`, `ocx_python`). The comment says bumping ocx "is still exactly one thing: bumping the external/ocx submodule pointer". `ocx-mirror/Cargo.toml` `[workspace] exclude = ["external/ocx"]`. Because patch tables do not travel with path dependencies, `[patch.crates-io]` for `oci-client`, `docker_credential`, `sigstore` is re-declared pointing into `external/ocx/external/...` (end of `ocx-mirror/Cargo.toml`). The comment there says dropping it "does NOT error, cargo would silently resolve the unpatched crates.io releases".
2. **Spawning the ocx binary.** `ocx-mirror/.claude/skills/update-ocx/SKILL.md` names two surfaces: "Linked crates" (moves with the submodule pointer) and "Spawned CLI, `ocx package push/announce/description`" (moves with the `ocx.sh/ocx/cli` pin in `ocx.lock`). The ocx interface-contract ADR counts "`ocx-mirror` spawns `ocx --format json` at 6 sites with tolerant structs and runs a released, pinned binary" (`ocx/.claude/artifacts/adr_ocx_interface_contract.md:55`). `ocx-mirror/ocx.toml` pins `ocx = "ocx.sh/ocx/cli:0.6.8"` in `[tools]` (test/dev) and a different `ocx = "ocx.sh/ocx/cli:0.6.8_20261115222333"` in `[group.release.tools]`.
3. **A deliberate non-link.** `ocx-mirror/Cargo.toml` and `AGENTS.md` say never to add a row for `ocx_cli` itself because linking it for two imports "cost 160 packages". The two things the mirror wanted from it (`tracing_init`, the TLS exit-code mapping) were copied into the mirror (`src/tracing_init.rs`, `ocx_mirror_error::tls_exit_code`), guarded by a unit test `tls_error_codes_match_ocx`.

Pinning and update procedure: the submodule commit is the pin (`git ls-tree HEAD external/ocx`), advanced by hand per `ocx-mirror/README.md:38-62` ("Bumping ocx") and the `/update-ocx` skill (a seven-plus-phase skill, including a Phase 0 preflight and a separate commit moving the CLI pin). A Renovate `git-submodules` manager is named in the ADR as the PR opener. Checklist items kept in sync by hand (`README.md:52-63`): `rust-toolchain.toml` channel against `external/ocx/rust-toolchain.toml`, Bazel toolchain version, the feature lists of shared dependencies ("copied EXACTLY from ocx's `[workspace.dependencies]`", `Cargo.toml`), the patch table, Bazel pins copied from ocx. Recent bump commits in `ocx-mirror`: `2870aa1` 2026-11-15 "chore(deps): bump external/ocx to c21f238b (2 commits)", `cd61ca8` 2026-11-02 "bump external/ocx to v0.6.6 and copy its shar...". Reading note: in the main checkout `git submodule status` shows the submodule uninitialised (leading `-`), pointer `9dd9e91e`.

Plugin dispatch: yes, in ocx. `ocx/crates/ocx_cli/src/app/plugin_dispatch.rs:4,63` ("Git-style plugin dispatch: `ocx <name> args` runs `ocx-<name> args` from PATH"). The ADR's registry layout point 5 says the binary is named `ocx-<name>` "so `ocx <name>` plugin dispatch ... works". The mirror's own documentation of the plugin relationship was not found (grep of `README.md`, `AGENTS.md` for "plugin" found nothing in `ocx-mirror`).

### How ocx tests against the mirror (the reverse direction)

`ocx/taskfiles/satellite.taskfile.yml` defines two tasks, both run from `ocx/.github/workflows/deep-repo.yml` (lines 185-299):

- `satellite:verify`: overlays the ocx checkout over a worktree of the mirror and builds the mirror (the ADR notes it "builds mirror only (no runtime parse) and is `continue-on-error`", `adr_ocx_interface_contract.md:55`). The contract ADR also adds a rule that the mirror workspace must resolve no internal-tier crate.
- `satellite:contract`: builds the candidate `ocx` from the ocx tree and runs the mirror's pytest suite against it, flooring at 6 passed `ocx_contract_site` tests; its SDK arm is skipped "until the mirror depends on `ocx-sdk`" (`satellite.taskfile.yml:247-252`).

How the mirror tests against ocx on its own side: its acceptance harness points `OCX_COMMAND`/`OCX_TEST_BINARY` style variables at the pinned released `ocx` from `ocx.toml` (`ocx-mirror/AGENTS.md`, `test/bazel_accept.sh` row: "points the harness at the Bazel-built `ocx-mirror` and the pinned `ocx`"). No version matrix was found; one pin plus the submodule pointer.

## 3. Scaffold: `ocx-mirror` compared with `ocx`

### Mechanism found: `ocx-sh/conventions`

- It is a copier template with two profiles, `full` and `mirror` (`conventions/README.md:36-38`; the `mirror` profile is for data repositories that mirror packages, not for the `ocx-mirror` tool, which uses `full`: `ocx-mirror/.copier-answers.yml` has `profile: full`).
- Ownership classes in `conventions/MANIFEST.toml`: "template-owned" (rewritten on each update, banner `ocx-baseline: template-owned file` on line 1) and "seeded-once" (`taskfile.yml`, `ocx.toml`, `grimoire.toml`, `AGENTS.md`, `.gitleaks.toml`, `.github/renovate-local.json`) (`conventions/README.md:43-48`). `task baseline:check` re-renders the recorded commit and fails on hand edits of template-owned paths.
- The interface adopting repos rely on is `conventions/docs/contract.md`: tasks (`setup`, `lock:check`, `baseline:check`, `release:prepare`, `copier:update`), required repo-owned root tasks (`verify`, `ci:lint`, `ci:test`, `ci:build`, `ci:docs`, `ci:deep`), workflow shapes, a composite setup action referenced as `ocx-sh/conventions/.github/actions/setup@<commit>`.
- Answers are recorded per repository: `ocx/.copier-answers.yml` is `_commit: v0.3.6`, `ocx-mirror/.copier-answers.yml` is `_commit: v0.3.5`. Both set `profile: full`, `bazel: true`, `submodules: true`; `ocx` adds `typescript` and a `site`.
- Fleet size: `conventions/fleet.toml` lists 126 repository entries, 104 with `adopted = true`, 101 at `iteration = 3` (mirror repositories). Iterations: 1 canary, 2 fleet, 3 mirrors (`fleet.toml:8`). Grimoire repositories are entries at iteration 2 with `adopted = false`: `grimoire-rs/grimoire` (answers `languages = ["rust","typescript"]`, `submodules = true`, `bazel = false`), `grimoire-rs/index`, `grimoire-rs/indexer`, and `ocx-sh/grimoire-lore` (`fleet.toml:104-160`).
- Drift is tracked by `conventions/drift/` plus a status page (`conventions/README.md:73-74`: `status.yml` publishes "the drift status page"; Pages must be enabled).
- Rules and skills are not copied by the template: "`skills/`, `rules/` | the `ocx-baseline` skill and rule, published with grim ... adopters install them, copier never copies them" (`conventions/README.md:62`). Each repository installs bundles from `grimoire.toml`.

### Comparison (tracked files, byte hash)

`ocx` tracks 4756 files, `ocx-mirror` 2042; 1426 paths exist in both; 1226 are byte-identical and 200 differ.

**Byte-identical in both** (counts of identical paths): `.claude/rules` 452, `.opencode/rules` 449, `.agents/skills` 101, `.claude/skills` 101, `.opencode/skills` 101 (these are grim-installed bundle outputs committed to the tree), plus `rustfmt.toml`, `.pre-commit-config.yaml`, `.taskrc.yml`, `CLAUDE.md`, `.bazelversion`, `opencode.json`, `LICENSE`, `taskfiles/baseline.taskfile.yml`, `.github/zizmor.yml`, `.github/workflows/claude.yml`, `.github/workflows/issues.yml`, `.github/workflows/canary.yml`, `.github/workflows/verify-version.yml`, `.github/ISSUE_TEMPLATE/{report,config}.yml`, `scripts/accept_shard.py`, `scripts/release_prune_select.py`.

**Template-owned files that differ only by version**: `.github/workflows/verify-basic.yml` differs by 12 lines: the setup-action pin (`@739ab84... # v0.3.6` against `@fe887c6... # v0.3.5`, five places) and two secrets the mirror's answers add (`BAZEL_CACHE_READ_AUTH`, `OTEL_OTLP_AUTH`). `cliff.toml` differs by 6 lines. `.copier-answers.yml`, `renovate.json`, `.gitleaks.toml` differ.

**Diverged repository-owned or ported files** (named, kind of divergence):

- `deny.toml` (85 lines in `ocx`, 36 in `ocx-mirror`): different advisory ignore lists; the `ocx` list is tied to its starlark and sigstore dependencies.
- `rust-toolchain.toml` (9 lines against 6): `ocx` adds Windows gnullvm targets; the mirror says "Keep the channel in sync with external/ocx/rust-toolchain.toml when bumping the submodule." (manual sync).
- `.gitleaks.toml` (66 against 18), `.claude/settings.json` (88 against 26), `.claude/taskfile.yml` (104 against 24), `.licenserc.toml`, `.gitmodules`, `.gitignore`, `README.md`, `AGENTS.md`, `Cargo.toml`/`Cargo.lock`, `MODULE.bazel`, `BUILD.bazel`, `.bazelrc`, `taplo.toml`, `ocx.toml`/`ocx.lock`, `grimoire.toml`/`grimoire.lock`: product-specific.
- `.github/workflows/`: `release.yml` (634 lines in `ocx`, 627 in `ocx-mirror`, 247 lines differ), `verify-deep.yml`, `deep-repo.yml`, `basic-repo.yml`, `prune.yml`, `backfill.yml`, `readiness-gate.yml`, `release-readiness.yml`, `acceptance.yml`. `.github/actions`: `bazel-cache-rc`, `release-ocx`, `test-telemetry` exist in both with differences (`ocx` also has `build-rust`, `ocx-install`, `release-assets`).
- `taskfiles/`: `bazel`, `release`, `rust`, `telemetry`, `git` taskfiles exist in both and differ; `ocx` has more (a `satellite.taskfile.yml`, others).
- `scripts/`: eight shared script names differ (`bazel_build_drift.py`, `bazel_test_floor.py`, `bazel_tag_guard.py`, `accept_floor.py`, `release_readiness_gate.py`, `backfill_release.py`, `bep_to_otlp.py`, `windows_local.sh`). The mirror's `AGENTS.md` describes many as "Verbatim ocx port" or "ported from ocx": the copies were made by hand and then diverged.
- `.claude/rules`: both have the 452 identical vendored files, but 43 differ and the repository-owned set differs. `ocx` has `arch-principles`, `subsystem-*` (cli, oci, package, tests, taskfiles, ci, website ...), `quality-*`, `workflow-github`; the mirror has `crate-placement`, `subsystem-mirror`, `security-threat-model`, `meta-plan-status`.
- `.claude/skills`: `ocx` carries the hex bundle (`hex-plan`, `hex-execute`, `hex-review`, `hex-finalize`, `hex-retro`, `hex-loop`, `nox-review`, ...). The mirror still has older hand-ported `architect`, `swarm-plan`, `swarm-execute`, `swarm-review`, `finalize`, `e2e-test`, and mirror-only `update-ocx`, `ocx-upstream-pr`. Cause visible in `grimoire.toml`: `ocx/grimoire.toml` lists bundles `hex` and `nox` (from `ghcr.io/michael-herwig/arcana`) and `typescript-essentials`; `ocx-mirror/grimoire.toml` lists only the six `*-essentials` bundles from `ghcr.io/ocx-sh/lore`.
- `.claude/agents`: 8 differing agent files; `ocx` has `worker-doc-writer.md`.
- `test/`: 23 differing files; the mirror has its own harness (`test/src/zot.py` starts per-session zot registries, `test/limits/<os>-release/`).
- Mirror-only: `.agents/memory/hex.md`, `docs/` plus `mkdocs.yml` (mkdocs-material), `OCX-CLI-RENAME-HANDOVER.md`, `CATALOG.md`, `release/`, `build.rs`, `tests/`, `crates/crate_map.toml`.

**Shared through a mechanism**:

| Item | Mechanism |
|---|---|
| Template-owned workflows, tasks, hooks, renovate preset, `cliff.toml` | copier from `ocx-sh/conventions` |
| Setup action | remote composite action `ocx-sh/conventions/.github/actions/setup@<sha>` |
| Rules and skills | grim bundles from `ghcr.io/ocx-sh/lore` and `ghcr.io/michael-herwig/arcana`, installed and committed |
| The ocx code | git submodule plus path dependencies (section 2) |
| Toolchain (task, uv, grim, zot, bazel) | `ocx.toml` and `ocx.lock` per repository |
| Bazel remote cache | `bazel-cache.ocx.sh/v1`, shared with ocx (`ocx-mirror/AGENTS.md` Layout table) |
| Reusable workflows via `workflow_call` | `verify-basic.yml` calls the repository-owned `basic-repo.yml`; `acceptance.yml` is called by `deep-repo.yml`; these are in-repository, not cross-repository |

Not found: a cross-repository reusable workflow other than the setup action, a GitHub template repository, a shared taskfile include by URL.

## 4. Recorded statements about the problem

**Decided**

- Split pattern, 2026-06-12, `adr_cli_repo_split_template.md` (Accepted). Quote: "Satellite can pin any ocx commit, including feature branches" and "ocx_lib API breaks discovered at submodule bump, not in ocx CI (accepted: satellites no longer gate ocx)". The second was later overtaken: from the interface-contract ADR (Accepted 2026-10-03) the mirror is a "lockstep" consumer and ocx CI does gate on it (`satellite:verify`, `satellite:contract`, `ocx/AGENTS.md:17`).
- Template extraction was a stated follow-up and was done differently: the ADR's checklist item 3 reads "Re-evaluate at n=2 (`ocx-mcp`): if diff vs ocx-mirror is pure name substitution, extract a GitHub template repo or `cargo generate` template; consider `ocx-sh/.github` shared workflows at n=3" (unchecked in the file). The `conventions` repository, copier-based, with first commits dated 2026-10-11, is what exists. The ADR file was not updated to say so (Status block and checklist item 3 unchanged).
- Item 4 of the same checklist: "Grimoire-distributed shared AI rules (replaces copied quality rules)" is unchecked in the ADR, and is in effect implemented (bundles in `grimoire.toml`, rules committed as grim output).
- ocx-mirror CI parity, 2026-11-13, `ocx-mirror/.agents/discussions/ocx-ci-parity.md` (state: handed-off to loop, ratified by the owner). Intent quote: "ocx-mirror was split out of the ocx mono-repo and is meant to track ocx's build and test conventions. ocx ... has since moved on and the mirror lags on four points". The four points: conventions version (mirror `v0.3.2`, ocx `v0.3.4`), Bazel Rust rules, test tiering, native acceptance on macOS and Windows. Goal quote: "the mirror matches ocx on all four points, taking ocx's files as the template wherever the mirror has no reason to differ." Executed as four PRs on one branch (`ocx-mirror/.agents/memory/hex.md:125-130`: P1 conventions v0.3.4, P2 zot, P3 rules_rs, P4 readiness).
- Interface boundary, 2026-10-03, `adr_ocx_interface_contract.md` (Accepted): the machine interface (JSON reports, error document, env vars, CLI grammar) becomes a gated, versioned contract; generated SDKs (`ocx-sdk` crate, `ocx.sh/ocx/sdkgen` package) are phase 2, and "publishing the `ocx-sdk` crate ... is one-way". Decision on the mirror in the same ADR: "`task satellite:contract` runs mirror's acceptance suite on them against the candidate binary, blocking". `satellite:contract` SDK arm is skipped because the mirror "does not depend on ocx-sdk yet (X5 moves it onto the SDK; needs a grant in the mirror repository)" (`satellite.taskfile.yml:252`).
- Crate dissolution (the `ocx_lib` split into the ecosystem crates): `ocx-mirror` adopted it in `983970c` 2026-09-20 "refactor(deps): adopt ocx's split crates in place of the d..." after `ocx` deleted `ocx_lib` at WP-37.

**Recorded as open**

- ADR checklist item 3 (extract template at n=2) and item 4 are unchecked in `adr_cli_repo_split_template.md`, though the work exists elsewhere (see above).
- The conventions repository records owner actions not yet done as of its README: create the Renovate GitHub App, set org secrets, enable Pages, "Tag `v0.1.0` and push" (`conventions/README.md:69-76`). Version tags later than `v0.1.0` are referenced by the adopters (`v0.3.5`, `v0.3.6`), so the last item may be stale; not verified.
- `ocx-mirror` runs behind `ocx` on conventions: `v0.3.5` against `v0.3.6` (answers files); the Windows target, the Bazel rules, and the hex bundle differ as listed in section 3.
- `ocx-sdk-python` consumes the CLI only: "probes plain `ocx version` and accepts any newer binary" (`adr_ocx_interface_contract.md:55`).
- The vendored rules in `ocx-mirror` that still say "Bumping ocx is one thing" (`Cargo.toml` comment) now span ten rows, which the file itself states ("the pointer bump now lands across those eight rows" and nine in the comment, ten in the code: the comment lags the rows).

**Tried and dropped (visible in the record)**

- `ocx_shell` and `ocx_announce` path rows: dropped because they are internal-tier ([ocx-sh/ocx#497](https://github.com/ocx-sh/ocx/issues/497), named in `ocx-mirror/AGENTS.md`); replaced by `--ci-annotations` on the push argv and a restated forge vocabulary in `crates/ocx_mirror_spec/src/forge.rs`.
- A row for `ocx_cli`: refused (160 packages).
- `rules_rust` Bazel setup in the mirror: replaced by `rules_rs` per the 2026-11-13 discussion (with two Windows patches deleted).
- Docker `registry:2` as the harness registry: replaced by per-session zot on macOS and Windows (same discussion).
- Early hand-written scaffold: the first ocx-mirror workflows (`oci-publish`, `deploy-dev`) were later dropped (`8a3fb71` 2026-11-13 "ci: drop deploy-dev and oci-publish").

**GUI, desktop application**: nothing found. A grep for `GUI`, `desktop app`, `tauri`, `electron` over `ocx/.agents`, `ocx/.claude/artifacts`, the mirror's discussions, memory and `AGENTS.md`, and `conventions` docs and `.agents` returned only files where the words occur in unrelated contexts (docs-baseline inventories, shim and toolchain ADRs, a signing ADR); none was a statement about a GUI product.

**Library API**: ocx's stance is "No `ocx_*` crate is a published library; the binary is the only consumer" (`ocx/AGENTS.md:15`), yet ten crates carry a lockstep "ecosystem" tier for a submodule consumer, and a generated-SDK route is planned for external programs. Compare grimoire's own file: "no stable *library* API, the binary is the only consumer" (`grimoire/uno/AGENTS.md`, What is Grimoire).

## 5. Friction evidence

Dated, in the record:

- 2026-09-16 to 2026-09-23, crate split: `ocx_lib` deleted in `ocx`; the mirror could not build until re-pointed. `ocx/.claude/artifacts/issue_drafts_crate_split_satellites.md` (Part A) states: "`ocx_lib` was deleted at WP-37. `ocx-mirror` links it by path and cannot build against this workspace until every import is re-pointed." Measured: "493 `ocx_lib` references across 138 files". Heaviest file `src/pipeline/target_registry.rs` 33 references. Also: "nothing here may be executed from this repository: both satellites are the owner's to change". Mirror-side commits: `4a1030e` 2026-09-23 "test: pin error messages and exit codes before the crate split", `983970c` 2026-09-20.
- 2026-09-24 retro (`ocx-mirror/.agents/retro/ledger/`): `ocx-upstream-pr-ocx-util-consumer-gate` (kind pitfall, medium): "mirror-only pub item in ocx_util passes local verify, reds ocx CI's pub-item-consumer gate", status fixed 2026-09-27. `ocx-upstream-pr-stale-bazel-lock` (pitfall, low): "ocx verify reds on a stale Cargo.bazel.lock.json after moving external/ocx; bootstrap does not repin", fixed.
- 2026-10-24 retro (`ocx/.agents/retro/ledger/code-docs-cleanup-template-owned-files.json`, report `2026-10-25.md` P-4): "A plan-ID comment sweep edited comment lines in two copier template-owned files (ocx-baseline banner); ... The fix needed its own restore commit." Status deferred.
- 2026-11-13 discussion `ocx-ci-parity.md`: the mirror lags four conventions and build points; the discussion's own count is conventions `v0.3.2` (mirror) against `v0.3.4` (ocx), two minor versions in about ten days.
- 2026-11-24, mirror retro inbox: `verify:scoped` omits `bazel:lint`, so a BUILD file edit "passed the scoped gate twice and went red only in CI's `ci:lint`"; and a second occurrence of linked-worktree friction in `hex-finalize` ("first: ocx inbox 2026-11-16"). Both are scaffold-level issues reported separately in each repository.
- Duplicate copies: the mirror's `src/tracing_init.rs` is "a verbatim copy" of ocx code, `ocx_mirror_error::tls_exit_code` "a copy of ocx's `impl ClassifyExitCode for TlsError`", feature lists of seventeen dependency rows are "copied EXACTLY" from `ocx/Cargo.toml` (each bump is a manual checklist, `README.md:52-63`). Both copies are deleted "only by the `ocx_tracing` extraction named in `src/tracing_init.rs`", an item not yet done.
- Conventions repository itself: commit `fe887c6` 2026-11-14 "fix(setup): the baseline bootstraps ocx 0.6.6 (#34)" and `bea938f` 2026-11-17 "fix(site): pin the website deploy action at v0.7.0" show the shared pieces being fixed in one place; whether the equivalent fixes had to be made twice before conventions existed was not measured.

## Negative results (looked for and not found)

- No statement about a GUI or desktop application in any of the three repositories.
- No crates.io-published ocx library and no plan to publish one other than the generated SDK crate.
- No version matrix in the mirror's CI against several ocx versions; one pinned binary plus a submodule pointer.
- No GitHub template repository, no cross-repository reusable workflow beyond the composite setup action, no shared remote taskfile include.
- No documentation of the `ocx mirror` plugin dispatch inside `ocx-mirror` itself.
- No ocx-side note that the conventions repository supersedes checklist item 3 of `adr_cli_repo_split_template.md` (ADR not updated).
- No `ocx-mcp` or `ocx-dist` satellite repository exists locally, so the ADR's "n=2" test (diff versus the mirror is pure name substitution) was never run in that form; the only second CLI satellite on disk is none. A `conventions/fleet.toml` entry `ocx-mirror-sdk` exists but has no local checkout.
- The `ocx-mirror` main checkout's submodule is uninitialised, so the exact pinned ocx commit's tree could not be read from there (the pointer hash was read from git).

## Follow-up leads

- Read `conventions/src/` and `conventions/MANIFEST.toml` in full for what a non-ocx adopter must answer, since grimoire repositories are already listed there with `adopted = false`.
- Read `conventions/drift/` output to see how big the real drift between `ocx` and `ocx-mirror` is by the checker's own measure.
- Read `ocx/taskfiles/satellite.taskfile.yml` and `ocx/scripts/satellite_contract.py` in full to see what a binary-contract gate looks like in practice.
- Check `ocx-sdk-python` and `ocx-mirror-sdk` on GitHub (not local) to learn how a binary-only consumer pins and tests ocx.
- Check the 2026-11-02 and 2026-11-15 `ocx-mirror` bump commits for how long one bump takes in practice.
