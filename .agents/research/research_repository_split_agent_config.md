# Research: First-party agent configuration inventory for the repository split

## Metadata

**Date:** 2026-11-25
**Domain:** devops
**Triggered by:** Two new repositories (`grim-mirror`, `grim-gui`) must adopt the agentic configuration in full, including everything that is not installed from a grim bundle.
**Expires:** 2027-05-25

## Direct Answer

This is an inventory, not a recommendation. Scope: the worktree `/home/mherwig/.herdr/worktrees/grimoire/uno` on branch `hex/artifact-mirroring`, plus a read-only look at `/home/mherwig/dev/grimoire-rs/vscode` and `/home/mherwig/dev/grimoire-rs/indexer`. Bundle-installed items are listed only to mark the boundary.

Classes:

- `as-is`: project-independent, can be taken unchanged.
- `pattern`: idea and structure carry over, content is written anew.
- `grimoire-only`: has no meaning outside this repository.

"Rust plugin" means the `grim-mirror` repository. "Tauri" means the `grim-gui` repository.

## 1. Boundary: what grim installs and what is first-party

`grimoire.toml` declares `clients = ["claude"]` and two bundles (`grimoire.toml:1-9`):

| Package | Source registry | Kind | Renders to |
|---|---|---|---|
| bundle `hex` | `ghcr.io/michael-herwig/arcana/hex:latest` | bundle of 10 skills and 1 rule | `.claude/skills/hex-*`, `.claude/rules/hex-state.md` |
| skills `hex-architect`, `hex-core`, `hex-discuss`, `hex-execute`, `hex-finalize`, `hex-init`, `hex-loop`, `hex-plan`, `hex-retro`, `hex-review` | same repository family `.../arcana/<name>` | skill | `.claude/skills/<name>/` |
| rule `hex-state` | `.../arcana/hex-state` | rule | `.claude/rules/hex-state.md` |
| bundle `nox` with skill `nox-review` | `ghcr.io/michael-herwig/arcana/nox:latest` | bundle, skill | `.claude/skills/nox-review/` |

`grimoire.lock` pins 12 packages and 2 bundles (`grimoire.lock` rows `[[skill]]` x11, `[[rule]]` x1, `[[bundle]]` x2).

Vendored but not in `grimoire.toml`: nine rules and four skills carry `license:` and `repository:` frontmatter and come from `ocx-sh/grimoire-lore` (`code-docs`, `docs-quality`, `python-packaging`, `python-quality`, `rust-cargo`, `rust-quality`, `typescript-packaging`, `typescript-quality`; skills `code-docs-cleanup`, `docs-instrument`, `docs-plan`, `docs-review`; `.claude/rules.md:121-135`). `hex-state.md` is vendored from `michael-herwig/arcana`. The test `TestVendoredArtifacts` hard-codes this set (`.claude/tests/test_ai_config.py:86-112`). Their support directories are excluded from context by `claudeMdExcludes` (`.claude/settings.json:92-99`).

Consequence for a new repository: the set of vendored rules and skills is the only part that a `grim install` of a different `grimoire.toml` reproduces. Everything below is first-party.

## 2. Entry files: `AGENTS.md` and `CLAUDE.md`

`CLAUDE.md` is 7 lines and only imports `@AGENTS.md` (`CLAUDE.md`; test `test_claude_md_is_a_thin_import_of_agents_md`, `test_ai_config.py:462`, limit 12 lines). `AGENTS.md` is 280 lines against a 280 line budget (`test_ai_config.py:479`).

| Item | Lines | Class | Evidence and new content |
|---|---|---|---|
| `CLAUDE.md` stub convention (edit `AGENTS.md`, never `CLAUDE.md`) | `CLAUDE.md:1-7` | as-is | No grimoire symbol. |
| `AGENTS.md` banner "Single project-context file" and `@`-import note | `AGENTS.md:1-11` | as-is | Generic. |
| `## What is Grimoire` | 12-25 | pattern | Names `grim`, 27 subcommands, 1.0.0 freeze. New: Rust plugin = what `grim-mirror` is and its Unstable status; Tauri = what the desktop shell is and that it only invokes `grim`. |
| `## Project Identity` (pointer to `product-context.md`) | 26-33 | pattern | New: product context per repo. |
| `## Rule Catalog` with `@.claude/rules.md` import | 34-41 | as-is | The structure is the reusable mechanism; text is generic. |
| `## Build & Development Commands` | 42-70 | pattern | `task`, `task verify`, `claude:tests`, `docs:check`, `upstream:stale`. New: Rust plugin = cargo, nextest, pytest acceptance suite; Tauri = npm or pnpm, `cargo tauri`, front-end lint and test. |
| `## Architecture` with path to subsystem-rule table | 71-98 | pattern | Names crates `grim-hook`, `grimoire-hook-core`, `grim-mirror`. New: one row per code area of each repo. |
| `## Environment Variables` | 99-118 | grimoire-only | Every row is a `GRIM_*` or vendor variable of `grim` (`AGENTS.md:101-117`). `grim-gui` needs at most a short list of its own. |
| `## First-Party Catalog` | 119-126 | grimoire-only | Names `catalog/`, `grim-usage`, `task catalog:verify`. |
| `## Core Principles` 1 to 8 | 132-172 | as-is | Generic. Principle 6 says "Never push to remote". |
| Principle 9 "Preserve Compatibility" | 173-178 | grimoire-only | Names the 1.0.0 freeze and `stability.md`. Test `test_principle_count_matches_headings` (`test_ai_config.py:485`) reads "N principles distill" and counts `### N.` headings, so a changed count needs a changed sentence. |
| `## Tech Stack` pointer | 180-186 | pattern | New: Rust plugin = Rust 2024, Tokio, Python pytest; Tauri = Tauri 2, Rust shell, TypeScript front end. |
| `## Workflow` (worktrees, commits, finalize, planning flow) | 187-213 | pattern | Names `../grimoire-wt-<topic>`, `.agents/worktrees/`, DCO sign-off, `task git:dco`. Test `test_worktrees_documented_as_ad_hoc` requires the string `grimoire-wt-` (`test_ai_config.py:511`). |
| `## Skills & Personas` | 214-229 | pattern | Lists this repo's skills. |
| `## Starting Work` | 230-239 | as-is | Points at the three workflow rules and `workflow-intent.md`. |
| `## Adversarial Review Guidance` | 240-259 | as-is | Generic; names `product-tech-strategy.md` only. |
| `# General Agent Safety` (non-interactive flags, session hygiene) | 260-280 | as-is | Generic. |

## 3. Rule catalog `.claude/rules.md` (217 lines)

| Item | Class | Evidence and new content |
|---|---|---|
| Purpose and "When to consult" (catalog exists because path-scoped rules fire only after a file is open) | as-is | `rules.md:1-14`. |
| Update protocol (any rule change updates the catalog in the same commit; structural tests enforce it) | as-is | `rules.md:16-24`; enforced by `test_catalog_covers_all_rules` and `test_catalog_references_resolve` (`test_ai_config.py:604,617`). |
| Table "By concern" | pattern | Rows name grimoire ADRs, `arch-threat-model.md`, `subsystem-artifact-kinds.md`. New: Rust plugin = security of git and registry input, CLI/JSON contract, mirror reconciliation; Tauri = IPC and capability scope, front-end quality, packaging and signing. |
| Table "By language" | pattern | Rows are Rust, Python, Bash, TypeScript; the new repos pick their own. |
| Table "By subsystem" (rule, path scope) | pattern | One row per `subsystem-*.md`; the idea is the point (see section 4.2). |
| Table "By auto-load path" | pattern | Mirrors every rule's `paths:` frontmatter by hand. |
| "Globals" footer | pattern | Names the four global rules; count is tested (`test_global_rule_count_matches`, `test_ai_config.py:917`, hard-codes `== 4`). |
| "Declared Path-Scope Overlaps" with exemptions | pattern | Every pair of rules sharing a `paths:` pattern must sit in a declared group, else `test_path_overlaps_declared_or_absent` fails (`test_ai_config.py:951`). Content is grimoire globs. |
| "Vendored rules" section with the nine vendored rules and "Open" question | pattern | Names two local edits to vendored files (shebang fixtures, `nav_depth.py`), grimoire specific. |
| "Skills by task topic" | pattern | New list per repo. |

## 4. `.claude/rules/` first-party files

Vendored files are excluded (see section 1). Hit column: count of grimoire-specific terms per file (`grim`, `GRIM_`, `.agents/`, `src/oci|install|command|catalog|store`, OCI, ocx) from a regex scan, against line count.

### 4.1 Always-loaded and generic rules

| File | Lines | `paths:` | Purpose | Class | Evidence |
|---|---|---|---|---|---|
| `quality-core.md` | 167 | none (global) | SOLID, DRY, KISS, YAGNI, severity tiers, verification honesty | as-is | 0 hits; claims itself shareable. |
| `product-tech-strategy.md` | 47 | none (global) | Golden-path tech choices | pattern | Names Tokio, mold, the `grim-hook` launcher deviation (`:19-27`), Python uv ruff, Trivy. New: Rust plugin = Rust, Python acceptance, no hook launcher; Tauri = Tauri 2, TypeScript toolchain, packaging. |
| `workflow-intent.md` | 37 | none (global) | Classify work, check GitHub, route to workflow | as-is | 5 hits, all `.agents/` and template paths that the new repos keep (`:30`). |
| `hex-state.md` | 24 | none (global) | Hex state lives in files | vendored | Installed from the `hex` bundle; present twice (project and `~/.claude/rules`). |
| `quality-rust.md` | 263 | `**/*.rs`, `**/Cargo.toml`, `**/Cargo.lock` | Rust quality | as-is | 0 hits. Test marks it shareable (`TestShareableQualityRules`, `:172-260`). |
| `quality-rust-errors.md` | 155 | `**/error.rs`, `**/errors.rs`, `**/*.rs` | Error type design | as-is | 0 hits. |
| `quality-rust-exit_codes.md` | 205 | `**/main.rs`, `**/exit_code.rs`, `**/*.rs` | CLI exit code design | as-is | 0 hits; central to a plugin CLI. |
| `quality-python.md` | 113 | `**/*.py`, `**/pyproject.toml`, `**/requirements*.txt` | Python quality | as-is | 0 hits. Co-fires with vendored `python-quality.md` (declared open question). |
| `quality-bash.md` | 149 | `**/*.sh`, `**/*.bash` | Bash safety | as-is | 2 incidental hits. |
| `quality-security.md` | 135 | `.github/workflows/**`, `.github/actions/**`, `.github/dependabot.yml` | Attack surfaces, OWASP and STRIDE checklist | pattern | Generic top; sections "Grimoire-Specific Attack Surfaces" and "Grimoire Audit Checklist" (`:64-135`) name `GRIM_AUTH_*`, `GRIM_INSECURE_REGISTRIES`, `com.grimoire.uncompressed.digest`. New: Rust plugin = git input, registry credential, spawned `grim`; Tauri = IPC commands, webview CSP, updater signing. |
| `workflow-bugfix.md` | 145 | `src/**`, `test/**`, `.claude/**`, `Cargo.toml`, `Cargo.lock` | Reproduce, RCA, failing test first, fix | as-is | 3 hits (`test/tests/` pytest path, `:36`). Glob changes with the layout. Carries the canonical Review-Fix Loop block. |
| `workflow-refactor.md` | 134 | same | Two Hats refactor flow | as-is | Mirror copy of the Review-Fix Loop block; `sync-canonical-blocks.py` re-syncs it and `test_review_fix_loop_parity` fails on byte drift (`test_ai_config.py:1335`). Both files travel together. |
| `workflow-feature.md` | 53 | `.agents/plans/**`, `.agents/adr/**`, `.agents/specs/**`, `.agents/research/**` | hex workflow, plan status tracking | pattern | 13 hits; names hex, `/hex-plan`, agent team workflow. Content mostly generic; new content = the repo's gates. |
| `workflow-git.md` | 105 | `CHANGELOG.md`, `cliff.toml`, `dist-workspace.toml` | Branching, two-phase commits, checkpoint convention | pattern | Section "Submodule Workflow (`external/`)" (`:93-99`) is grimoire-only; rest generic. Glob targets release files, which differ per repo. |
| `workflow-github.md` | 190 | `.github/ISSUE_TEMPLATE/**` | Issue types, labels, issue and PR protocol | pattern | Names org `grimoire-rs`, issue types, `area/registry`, `area/oci` (`:29,44,50,72`). New: labels and issue types of the new org or repository. |
| `meta-ai-config.md` | 293 | `.claude/**` | Governs `.claude/` artifacts: three activation layers, context budget, artifact conventions, Plan Status Protocol, learnings store, structural tests | pattern | 15 hits. Hard-codes "4 global rules", the 280 line budget, the learnings `category` enum containing `mirror`, ADR paths. Idea carries; numbers and names are edited. |

### 4.2 The subsystem family (the idea named by the owner)

The idea: one rule per code area, `paths:` scoped to that area's files, structured as types, invariants, gotchas, cross-references, under 200 lines. Five mechanisms make the idea work, and each is first-party:

1. The path-to-rule table in `AGENTS.md` "Architecture" (`AGENTS.md:71-98`).
2. The catalog tables "By subsystem" and "By auto-load path" in `rules.md`, kept in step by tests (`test_catalog_subsystem_coverage`, `test_ai_config.py:748`: every `subsystem-*.md` named in `AGENTS.md` must appear in the catalog).
3. Declared overlaps for rules that share a glob (`rules.md` "Declared Path-Scope Overlaps"; `test_path_overlaps_declared_or_absent`).
4. Dead-glob detection (`test_all_rule_globs_match_files`, `:411`; `test_subsystem_globs_not_catch_all_rs`, `:437`).
5. The post-tool-use reminder `CONTEXT_REMINDERS` that tells the agent to update the matching rule after editing a subsystem path (`post_tool_use_tracker.py:31-45`), plus the skill `meta-validate-context` that audits freshness.

| File | Lines | `paths:` | Purpose | Class | Evidence |
|---|---|---|---|---|---|
| `subsystem-mirror.md` | 32 | `crates/grim-mirror/**` | Invariants of the mirror crate: no dependency on `grimoire`, git hardening, caps, licence gate, report contract, acceptance layout | pattern | Counterpart exists for the Rust plugin (section 9). Its content moves with the crate, but invariant 1 (`:11`, "must move to its own repository unchanged"), the `test/tests/test_mirror_*.py` testing section (`:25`) and the cross-references (`:30-32`) are rewritten after the split. |
| `subsystem-cli.md` | 70 | `src/**`, `crates/**` | CLI shell: clap, global flags, dispatch | pattern | 2 hits. Rust plugin = its own CLI shell and exit codes; Tauri = none. |
| `subsystem-cli-api.md` | 136 | `src/**`, `crates/**` | JSON output formats and report contract | pattern | Rust plugin = its run report (`schema`, `items[]`); Tauri = the JSON it parses from `grim`. |
| `subsystem-cli-commands.md` | 107 | `src/**`, `crates/**` | Command index | grimoire-only | 216 hits; index of grim's 27 subcommands. |
| `subsystem-file-structure.md` | 102 | `src/**`, `crates/**` | Content store and index layout | grimoire-only | Names `$GRIM_HOME` store layout. |
| `subsystem-ci.md` | 247 | `.github/workflows/**`, `.github/actions/**`, `.github/dependabot.yml` | CI workflows | pattern | 13 hits; names grim workflows. New: per-repo CI (Rust plugin = registry service in CI; Tauri = cross-platform bundling). |
| `subsystem-taskfiles.md` | 167 | `taskfile.yml`, `taskfiles/**/*.yml`, `**/taskfile.yml` | Taskfile conventions: two-phase verify, caching, `REPORTS_DIR`, dot-prefix helpers | pattern | Conventions generic (`:79-96`); file-layout table (`:11-24`) and verify phases (`:28-33`) are grimoire's. |
| `subsystem-tests.md` | 127 | `test/**` | Acceptance suite, fixtures, registry fixture, suite floor | pattern | 58 hits; names `GrimRunner`, `SUITE_FLOOR`. New: Rust plugin = pytest against an OCI registry and `grim`; Tauri = front-end unit tests plus a shell smoke test. |
| `subsystem-artifact-kinds.md` | 77 | `src/oci/artifact_kind.rs` and 6 more paths | Artifact kinds x interfaces matrix | grimoire-only | Names `ArtifactKind`, sibling repos. |
| `subsystem-client-detection.md` | 134 | `src/install/target.rs`, `client_target.rs` | Client detection | grimoire-only | grim installer. |
| `subsystem-config-keys.md` | 113 | `src/command/config_keys.rs`, `src/config/declaration.rs` | Config key title style | grimoire-only | grim config. |
| `subsystem-install-layout-agents-mcp.md` | 118 | five `src/install/*` files | Agents and MCP render | grimoire-only | grim installer. |
| `subsystem-install-layout-hooks.md` | 142 | `src/hook/**`, `crates/grim-hook/**`, `crates/grimoire-hook-core/**` | Hook install and launcher | grimoire-only | grim hooks. |
| `subsystem-install-layout-mods.md` | 40 | `src/install/mod_*.rs` | Mods render | grimoire-only | grim installer. |
| `subsystem-install-layout-settings.md` | 121 | `src/install/settings_*.rs` | Settings render | grimoire-only | grim installer. |
| `subsystem-install-layout-skills-rules.md` | 153 | four `src/install/*` files | Skills and rules render | grimoire-only | grim installer. |
| `subsystem-install-state.md` | 122 | `src/install/install_state.rs`, `src/path_safety.rs` | Install state, path containment | grimoire-only | grim installer. |
| `subsystem-path-anchors.md` | 166 | `src/install/path_anchor.rs` | Path anchors | grimoire-only | grim installer. |
| `subsystem-vendor-paths.md` | 165 | `src/install/vendor*.rs` and others | Global roots per AI client | grimoire-only | grim installer. |

### 4.3 Architecture, product, docs and release rules

| File | Lines | `paths:` | Purpose | Class | Evidence |
|---|---|---|---|---|---|
| `arch-principles.md` | 137 | `src/**/*.rs`, `crates/**/*.rs` | Design principles, crate layout, command flow, ADR Index table, code style, where features land | pattern | 225 hits. The ADR Index table is tested: every ADR under `.agents/adr/` needs a row (`TestAdrIndex`, `test_ai_config.py:1937`). New: per-repo layout and ADR index. |
| `arch-threat-model.md` | 212 | `src/oci/**`, `src/install/**`, `src/command/hook*`, login, logout, publish, release, `catalog/**`, `crates/**` | Attackers T1 to T5 in scope, non-goals N1 to N5, invariants I1 to I6 | pattern | 61 hits. New: Rust plugin = attackers on upstream git content and the registry; Tauri = attackers on webview content and IPC. |
| `product-context.md` | 174 | `docs/**`, `.agents/plans|adr|specs|research/**`, four skills | Canonical product identity, users, related repos, comparable tools, research keywords; has an update protocol at the bottom | pattern | Section "Related Repositories" (`:117`) must list the new repos. |
| `docs-style.md` | 125 | `docs/**` | Grimoire docs voice | grimoire-only | Problem-solution voice about Grimoire (`:25`) and the watchlist symbols (`:92`); neither new repository has a docs site in the brief. |
| `vendor-capability-watchlist.md` | 104 | `src/install/vendor_*.rs` and docs pages | Re-verify upstream before patching a vendor decline | grimoire-only | Data in `.agents/upstream-watchlist.md`. |
| `workflow-release.md` | 181 | `dist-workspace.toml`, `cliff.toml`, `CHANGELOG.md`, three workflows | Release ceremony | grimoire-only | cargo-dist, ocx signing, catalog publish. |

## 5. `.claude/hooks/`

Wiring is in `.claude/settings.json` (section 6). Language: Python, stdlib only, run with `uv run`, PEP 723 header (`test_all_hooks_have_pep723_header`, `:824`). Zero context cost.

| Hook (lines) | Event, matcher | Enforces or reminds | Hard-coded repository facts | Class |
|---|---|---|---|---|
| `hook_utils.py` (688) | library | `StateManager` (locks under `.claude/hooks/.locks`, state under `.claude/hooks/.state`), `LearningsStore` (`.claude/state/learnings.jsonl`, pending queue, orphan file, TTL 90 days, decay), `deny()` output | `ttl_seconds=300` for the stamp (`:289`); category enum incl. `mirror` (per `meta-ai-config.md`); path names above | pattern |
| `pre_commit_verification.py` (341) | PreToolUse `Bash` | Denies `git commit` unless a stamp younger than 300 s exists; resolves the worktree being committed (`git -C`, `cd`); lets commits in a foreign repository pass | Message "Run `task verify` first" (`:296`); `detect_project_tools` knows Cargo, package.json, pyproject, go.mod (`:224-287`); stamp written by root `taskfile.yml:80-85` (`.verify:mark`, `date +%s > .claude/hooks/.state/commit-verified`) | pattern |
| `conventional_commit_validator.py` (116) | PreToolUse `Bash` | Blocks commit messages that do not match Conventional Commits | Type list `feat|fix|refactor|ci|chore|docs|test|perf|build|style|release` (`:21-23`), `release` from `task release:prepare` | pattern |
| `pre_tool_use_validator.py` (194) | PreToolUse `Write\|Edit\|MultiEdit` | Secret patterns, protected paths, file locks for concurrent agents | `_PROTECTED_PATTERNS = (".git/", ".env", ".mcp.json")` (`:65`) | as-is |
| `post_tool_use_tracker.py` (360) | PostToolUse `Edit\|MultiEdit\|Write` | Prints a reminder naming the rule or doc to update; samples context budget | `CONTEXT_REMINDERS` (13 rows, `src/**` to `subsystem-*.md`, `:31-45`); `CONFIG_REMINDERS` (23 rows, `:48-190`) naming `taskfile.yml`, `docs/src/content/docs/*.md`, `catalog/skills/*`, `src/oci/hook.rs`, `deny.toml`, `.agents/adr/adr_*.md`, `.agents/plans/plan_*.md`, `.agents/upstream-checks.md`; `_AI_CONFIG_MD_PREFIXES` (`:198-202`) | pattern |
| `user_prompt_router.py` (144) | UserPromptSubmit | Reads `triggers:` from each `.claude/skills/*/SKILL.md` and suggests a skill in one line | None; reads frontmatter at runtime (`:1-12`) | as-is |
| `session_start_loader.py` (66) | SessionStart `startup\|resume` | Reports concurrent agents and a handoff message | None | as-is |
| `stop_validator.py` (98) | Stop | Releases locks, merges pending learnings, reminds about uncommitted files | None beyond the learnings paths | as-is |
| `subagent_stop_logger.py` (102) | SubagentStop | Logs completion, captures `[LEARNING]` JSON from subagent output | None beyond the learnings paths | as-is |
| `pre_push_main_blocker.py` (105) | none: not wired | Would block `git push` to `main` or `master` | Branch names `main`, `master` | as-is (but unwired) |

The learnings store is the one subsystem inside hooks: schema v1 in `meta-ai-config.md` "Cross-Session Learnings Store" and `.agents/adr/adr_ai_config_cross_session_learnings_store.md`; categories `rust, python, ts, oci, test, clippy, mirror, build, other`.

## 6. Settings, MCP and local config

| Item | Structure | Class | Evidence |
|---|---|---|---|
| `.claude/settings.json` (100 lines) | `enableAllProjectMcpServers: true`; `hooks` for SessionStart, PreToolUse (two matchers), PostToolUse, Stop, SubagentStop, UserPromptSubmit, each `uv run "$CLAUDE_PROJECT_DIR/.claude/hooks/<file>"` with 5 to 10 s timeouts; `enabledPlugins` (`frontend-design@claude-plugins-official`); `claudeMdExcludes` for six vendored rule support directories (`:89-99`). No `permissions` block, no `env` block, no secrets. | pattern | Wiring and timeouts carry over; `claudeMdExcludes` list and the plugin differ per repo. `pre_push_main_blocker.py` is absent from the wiring. |
| `.claude/settings.local.json` | Gitignored (`.gitignore:21`); no template exists | as-is | Convention only. |
| `.mcp.json` (303 bytes) | Two servers: `github`, `context7`. The protected-path list in `pre_tool_use_validator.py` blocks edits to it. | pattern | New repos choose their own servers. `grim-gui` may want none. |
| `.serena/` (`project.yml`, `memories`, cache) | Serena MCP project config; cache gitignored (`.gitignore:88`) | pattern | Per-project language setup. |
| `.taskrc.yml` | `interactive: true`, `failfast: true` | as-is | 2 lines. |
| `.gitignore` agent entries | `CLAUDE.local.md`, `.claude/settings.local.json`, `.claude/scheduled_tasks.lock`, `.claude/hooks/.file-tracker.log`, `.claude/hooks/.locks/`, `.claude/hooks/.state/`, `.claude/state/`, `.claude/reports/`, `.serena/`, `.task`, `.agents/worktrees/`, `.agents/.tmp/`, `.agents/*.lock` (`.gitignore:18-40,86,110-112`) | as-is | Required by `test_gitignore_contains_state_dir` (`test_ai_config.py:1269`). |
| `.github/CODEOWNERS` | One line `* @michael-herwig`; no agent-config entry | as-is | Nothing agent specific. |
| `.gitattributes`, `.licenserc.toml` | No agent-config entries found | as-is | grep for `claude\|agents` returned nothing. |

## 7. `.claude/tests/` (structural tests)

Run by `task claude:tests` (`uv run --directory tests pytest test_ai_config.py test_upstream_stale.py`). `task claude:verify` = tests plus `lint:links` (lychee, skipped if absent) (`.claude/taskfile.yml:23-27`). `test_hooks.py` and `test_workflows.py` are not in the `tests` task command list.

| File and class | What it checks | Repository-specific facts hard-coded | Class |
|---|---|---|---|
| `test_ai_config.py` `TestVendoredArtifacts` (`:117`) | Vendored set equals the declared set | `VENDORED_RULES` (9 names) and `VENDORED_SKILLS` (15 names) (`:86-112`) | pattern |
| `TestShareableQualityRules` (`:172`) | Shareable rules carry no grimoire names; all quality rules have `paths:` | `_GRIMOIRE_FORBIDDEN_STRINGS` (10 type names such as `PackageErrorKind`, `GrimRunner`), `_SHAREABLE_RULES` (4 files) (`:181-197`) | pattern |
| `TestSkillsLayout` (`:279`) | Flat skill layout, dir name equals frontmatter name, no category dirs, action skills set `disable-model-invocation` | None | as-is |
| `TestRuleGlobs` (`:387`) | Every `paths:` glob matches a file; subsystem globs not catch-all `*.rs` | None | as-is |
| `TestContextMd` (`:459`) | `CLAUDE.md` stub, `AGENTS.md` at most 280 lines, "N principles" count, `grimoire-wt-` string | 280, `grimoire-wt-` (`:511`) | pattern |
| `TestFeatureWorkflow` (`:533`) | hex workflow step numbers sequential | None | as-is |
| `TestArtifactPaths` (`:563`) | `security-auditor` uses `.agents/` not `./artifacts/` | Skill name `security-auditor` | grimoire-only |
| `TestRuleCatalog` (`:588`) | Catalog exists, covers all rules, references resolve, all markdown refs resolve, subsystem coverage | Catalog path `.claude/rules.md` | as-is |
| `TestReleaseImplementation` (`:773`) | Workflow filenames in `workflow-release.md` exist | File `workflow-release.md`, `.github/workflows/` | grimoire-only |
| `TestHookScript` (`:813`) | Hooks are Python, PEP 723 header, `CLAUDE_PROJECT_DIR`, PostToolUse try/except | None | as-is |
| `TestTaskfileLint` (`:863`) | Shell lint task handles no scripts | Task name in the root taskfile | pattern |
| `TestAiConfigOverhaulPhase1` (`:890`) | Workflow rules have `paths:`, global rule count equals 4, path overlaps declared | `== 4` (`:917-950`), exemption list of declared groups | pattern |
| `TestAiConfigOverhaulPhase2` (`:1009`) | Skill descriptions follow the CSO policy, YAML validity, description budget cap, `disable-model-invocation` intent | 4000 character description cap, forbidden verbs, ADR `adr_ai_config_skill_description_csopolicy.md` | pattern |
| `TestAiConfigOverhaulPhase4` (`:1261`) | `.gitignore` holds `.claude/state/`; `meta-ai-config.md` has the learnings section | Section name | as-is |
| `TestAiConfigOverhaulPhase5` (`:1305`) | Review-Fix Loop parity between `workflow-bugfix.md` and `workflow-refactor.md`; skill body under 500 lines | The two carrier files (`:1305-1334`) | as-is |
| `TestPromptRoutingTriggers` (`:1426`), `TestUserPromptRouter` (`:1569`) | `triggers:` present, unique, discriminating; router registered in settings, exits 0, one-line output | None | as-is |
| `TestPlanStatusBlock` (`:1688`) | Every `plan_*.md` has a Status block with four fields in the first 30 lines; templates carry it | `.agents/plans/` | as-is |
| `TestAdrIndex` (`:1815`) | Every ADR has a row in the `arch-principles.md` ADR Index; a finalized plan's ADR is not left Proposed | `arch-principles.md`, `.agents/adr/` | pattern |
| `test_hooks.py` (1398 lines, 15 classes) | Unit tests for hook behavior (stamp TTL, foreign repository, conventional commit, push blocker, tracker, learnings) | `task verify` message, type list, reminder rows | pattern |
| `test_upstream_stale.py` (520) | The `upstream_stale.py` script of `upstream-refresh` | `.agents/upstream-checks.md` format | grimoire-only |
| `test_workflows.py` (64) | OCX signing identity: `id-token: write` sites, `allow-dirty` | `release-ocx.yml`, `dist-workspace.toml` (`:1-30`) | grimoire-only |
| `pyproject.toml` | pytest dependency set for the tests | None | as-is |

## 8. Templates, scripts, state

| Item | Content | Class | Evidence |
|---|---|---|---|
| `.claude/templates/artifacts/` | 11 templates: `adr`, `bugfix_plan`, `design_spec`, `plan`, `postmortem`, `pr_faq`, `prd`, `research`, `roadmap`, `security_audit`, `system_design` (all `*.template.md`). `plan` and `bugfix_plan` carry the Status block (tested). | as-is | Used via `.agents/memory/hex.md` Pointers ("the project's own, not hex's fallbacks"). |
| `.claude/templates/claude_mechanisms/` | `command`, `rule`, `skill` templates and `hook.template.sh` | as-is | Used by `meta-maintain-config`. |
| `.claude/scripts/statusline.sh` (285), `statusline-install.sh` (24), `README.md` (67) | Cross-platform status line; installs to `~/.claude/` | as-is | Personal tooling; `README.md:3` names "Grimoire". |
| `.claude/scripts/sync-canonical-blocks.py` (56) | Copies the Review-Fix Loop block from `workflow-bugfix.md` to `workflow-refactor.md` | as-is | Needs the two files; task `claude:fix:canonical-block`. |
| `.claude/taskfile.yml` (83) | Tasks `verify`, `tests`, `lint:links`, `fix:canonical-block`, `statusline:install`; `x-shared` source anchor; lychee exclusions | pattern | Source list names `../taskfile.yml`, `../taskfiles/**`, `../.github/workflows/**`, `../.agents/plans/*.md`; lychee excludes vendored paths (`:63-78`). Included from root `taskfile.yml:19-21` as `claude:`. |
| `.claude/state/` | Gitignored; holds `current_plan.md`, `learnings.jsonl`, `subagents.jsonl` (written by a global hook per the model-routing policy), per-goal resume files | as-is | Per-worktree ephemera. |
| `.claude/skills/*/` support directories | `rules/<vendored>/` directories excluded from context | pattern | See settings. |

## 9. First-party skills (`.claude/skills/`, no bundle provenance)

Provenance check: `license:` frontmatter absent. 17 skills.

| Skill (lines) | Purpose | Embeds | Class |
|---|---|---|---|
| `commit` (162, plus `commit_reference.md`) | Working-phase checkpoint commits | "Grimoire" posture; `task verify`; DCO | pattern |
| `finalize` (195) | Rewrite checkpoint commits into Conventional Commits | `current_plan.md`, plan Status protocol; refuses unless Step is `finalized` | pattern |
| `next` (198) | Print the next slash command from plan state | `.agents/plans`, `current_plan.md` | pattern |
| `bugfix` (126) | Failing-test-first bug workflow | `task rust:verify`, `test/tests` | pattern |
| `rca` (84, `references/`) | Read-only root cause verdict on a ticket | `.agents/adr`, cargo build, pytest, `grimoire-rs` repo | pattern |
| `fix` (50) | Ticket to approved fix plan | `cargo test`, pytest, `task rust:verify` | pattern |
| `builder` (61) | Typed implementation work | `cargo check`, `cargo fmt`, pytest, `src/`, `test/` | pattern |
| `qa-engineer` (62) | Test design and acceptance tests | pytest, cargo, Task Runner section | pattern |
| `code-check` (77) | Quality audit | None | as-is |
| `security-auditor` (49) | Security audit, threat model; artifacts to `.agents/security_audit_*` | `.agents/` | as-is |
| `docs` (46) | Author Grimoire documentation | "Grimoire", `docs/` | grimoire-only |
| `changelog` (99) | Site changelog page per release | `src/content/docs/changelog`, `upgrading` | grimoire-only |
| `prepare-release` (144, `references/`) | Release preparation | `cargo set`, `grim release`, `grim x`, catalog and docs drift | grimoire-only |
| `outside-pr-review` (141, `scripts/`) | Security-first review of fork PRs | `src/command/`, pytest, hidden-character scripts | pattern |
| `upstream-refresh` (184, `references/`, `scripts/`) | Upstream knowledge freshness; writes `.agents/upstream-checks.md` | `grim depends`, `.agents/upstream-watchlist.md`, vendor harness domains | grimoire-only |
| `meta-maintain-config` (158) | Create and edit rules, skills, hooks under `.claude/` | `.agents/research` | as-is |
| `meta-validate-context` (71) | Audit `subsystem-*.md` freshness against code | `src/module_name/` placeholder | as-is |

Skill conventions (as-is): frontmatter `description` follows CSO, `triggers:` required for user-invocable skills (3 to 7 literal phrases, unique across skills), `disable-model-invocation: true` for action skills, body under 500 lines, no category directories (`meta-ai-config.md` "Skills").

## 10. The `.agents/` tree as a convention

`.agents/README.md` is 7 lines. Committed directories: `adr/` (62 files), `plans/` (92, `plans/archive/` 3), `specs/` (15), `research/` (222), `discussions/` (18), `goals/`, `review/`, `discovery/`, `memory/`, `retro/`, `skills/`, `worktrees/` (gitignored). Root has one-off records (`handover_*.md`, `review_*.md`, `security_audit_*.md`, `upstream-checks.md`, `upstream-watchlist.md`, `upstream-followups-*.md`). `.gitignore:107-112` keeps `.agents/memory|plans|adr|research|specs` committed and ignores `worktrees/`, `.tmp/`, `*.lock`.

| Item | Class | Evidence and new content |
|---|---|---|
| Directory convention (`adr`, `plans`, `specs`, `research`, `discussions`, `goals`, `memory`, `retro`, `worktrees`) | as-is | Named by `AGENTS.md:187-213` and by `hex.md` Pointers. |
| Planning flow ADR, Design Spec, Plan, Implementation | as-is | `AGENTS.md` "Workflow". |
| Plan `## Status` block (Plan, Active phase, Step, Last update; optional `Parent plan`), pointer `.claude/state/current_plan.md`, per-skill mutation table | as-is | `meta-ai-config.md` "Plan Status Protocol"; tested by `TestPlanStatusBlock`. |
| `.agents/memory/hex.md` (976 lines) | pattern | Structure as-is: `## Pointers`, `## Preferences` (YAML: `models`, `adversary`, `perspectives.always`, `research-axes`), `## Memory`. |
| `hex.md` Pointers content | pattern | Names `task verify`, `docs:check`, grimoire ADR conventions, security-sensitive paths `src/oci/**`, "Constitution: nine principles" (`hex.md:8-51`). New: Rust plugin = `cargo` gate, registry and git paths as security-sensitive; Tauri = IPC and updater paths. |
| `hex.md` Preferences, project-neutral | as-is | `models` mapping (`fast-balanced: sonnet`, `deep-reasoning: opus`, overrides), `adversary: codex:rescue`, Sol 6.1 note (`hex.md:55-105`). |
| `hex.md` Preferences, project-specific | grimoire-only | `perspectives.always` rows on `src/oci/**`, `src/command/{login,logout}.rs`, `src/command/{publish,release}*`, `catalog/**`; `research-axes` (OCI spec, vendor layouts) (`:69-92`). |
| `hex.md` `## Memory` entries (about 870 lines) | grimoire-only | Dated plan and ADR history of this repo. A new repository starts with an empty Memory. |
| `.agents/retro/` (inbox, plans) | as-is | Retro inbox per `hex-state.md`; JSON entries. |
| `.agents/discussions/`, `.agents/goals/` | as-is | hex-discuss and hex-loop artifacts; contents grimoire-only. `repository-split.md` is the open discussion. |
| `.agents/upstream-watchlist.md`, `.agents/upstream-checks.md`, `upstream-followups-*.md` | grimoire-only | Harness capability ledger. |
| `.agents/skills/` (15 vendored skill copies: hex-*, docs-*, code-docs-cleanup, nox-review) | as-is | Vendored; generic client path `.agents/skills`. Not authored here. |
| `.agents/adr/` ADR set and `arch-principles.md` ADR Index | pattern | Convention carries (index row required per ADR); the 62 ADRs are grimoire. |
| One-off root records | grimoire-only | `handover_*`, `review_*`, `security_audit_*`. |

## 11. Other agent clients

| Location | Finding |
|---|---|
| `opencode.json`, `.codex/`, `.cursor/`, `.github/copilot-instructions.md` | Not present in the worktree root. |
| `AGENTS.md` | Written for every `AGENTS.md`-aware agent (banner `AGENTS.md:1-10`); Claude reaches it through `CLAUDE.md`. |
| `.agents/skills/` | Holds 15 skill copies. The worktree has no `grimoire.toml` entry that renders there. |
| `grimoire.toml` `[options] clients` | `["claude"]` only (`grimoire.toml:2`). |
| `.ocx/toolchain`, `ocx.toml`, `ocx.lock` | Toolchain pinning by ocx, not an agent client. |
| `nox-review` skill and `adversary: codex:rescue` | Second-harness review runs headlessly from an ephemeral worktree with the repository's own instruction and hook files removed (`nox-review` description); not a client config. |

## 12. Other items that shape agent work

| Item | Class | Evidence |
|---|---|---|
| Commit guard (stamp from `task verify`, 300 s) | pattern | Needs a root task `.verify:mark` and the hook. `MEMORY.md` records that it also fires in sibling repos' commits unless the foreign-repository shape matches. |
| `.github/ISSUE_TEMPLATE/**` governed by `workflow-github.md` | pattern | Forms differ per repository. |
| `CONTRIBUTING.md`, `SECURITY.md`, `CODE_OF_CONDUCT.md` | pattern | Root files; contents grimoire. |
| `task` targets that bind agent config: `claude:verify` inside root `verify` (`taskfile.yml:93`) | pattern | The gate that makes catalog drift a red build. |
| `.config/nextest.toml` | pattern | Rust test runner config. |
| `docs/` site, `catalog/` | grimoire-only | Referenced by 8 reminder rows. |
| DCO sign-off (`git commit -s`, `task git:dco`) | pattern | `AGENTS.md:191-193`; the `git:dco` task and CI check must exist in the new repository. |
| Submodule `external/rust-oci-client` | grimoire-only | `workflow-git.md:93-99`. |

## 13. Counterparts for the existing `crates/grim-mirror` work

| First-party item | Has a counterpart for grim-mirror | Evidence |
|---|---|---|
| Subsystem rule | Yes: `subsystem-mirror.md` (32 lines, `paths: crates/grim-mirror/**`) | `.claude/rules/subsystem-mirror.md`; catalog rows in `rules.md` ("By concern", "By subsystem", "By auto-load path") and the `AGENTS.md` architecture table. Last commit `eaa7da0b`. |
| Architecture entry in `AGENTS.md` | Yes | `AGENTS.md:71-98` names `crates/grim-mirror` (separate binary, no dependency on the main crate, source-only, Unstable). |
| ADR and design | Yes | `.agents/adr/adr_artifact_mirroring.md`, `.agents/specs/design_artifact_mirroring.md`, plan `.agents/plans/plan_artifact_mirroring.md`, 7 research files, discussion `artifact-mirroring.md`, goal file. |
| Acceptance tests | Yes | `test/tests/test_mirror_*.py` (10 files) plus `test_publish_mirror_table.py`, fixture `grim_mirror_binary`; invariant 1 pinned by `crates/grim-mirror/tests/boundary.rs`. |
| Learnings category | Yes | `mirror` in the enum (`meta-ai-config.md`). |
| `CONTEXT_REMINDERS` row for `crates/grim-mirror/**` | No | The table has only `src/**` rows (`post_tool_use_tracker.py:31-45`); the `crates/**` rows are absent for all crates. |
| `CONFIG_REMINDERS` row for the crate's `Cargo.toml` or the mirror docs page | No | Only the root `Cargo.toml` row (`:145-149`). |
| `docs/src/content/docs/mirroring.md` reminder | No | Not in `CONFIG_REMINDERS`. |
| `quality-*.md`, `arch-threat-model.md` coverage | Partial | `arch-threat-model.md` globs `crates/**`; `quality-rust*.md` cover `**/*.rs`. |
| Product-context entry for mirror | Not checked as a section; `product-context.md` has a "Related Repositories" section (`:117`) | Not read in full. |
| Own `CLAUDE.md` or `AGENTS.md` | No | The crate has none; it inherits the root. |

## 14. Sibling checkouts (read-only)

| Item | `vscode` (`/home/mherwig/dev/grimoire-rs/vscode`) | `indexer` (`/home/mherwig/dev/grimoire-rs/indexer`) |
|---|---|---|
| `AGENTS.md` | 12 lines, short build and rule list; says CLAUDE.md is authoritative | absent |
| `CLAUDE.md` | 70 lines of real content (not a stub) | 11 lines, imports `@.claude/rules.md` only (rule catalog discoverability) |
| `.claude/rules.md` catalog | absent | present, committed |
| `.claude/rules/` | 9 files: `artifact-kinds.md`, `grim-compat-markers.md`, `plan-status.md`, `quality-bash.md`, `quality-core.md`, `quality-security.md`, `quality-typescript.md`, `workflow-git.md`, `workflow-swarm.md` | `artifact-kinds.md`, `grim-compat-markers.md`, vendored `css-theming*`, `typescript-packaging.md`, `typescript-quality*` |
| Subsystem family | none | none |
| `.claude/hooks/` | empty directory | empty directory |
| `.claude/settings.json` | absent | present: only `claudeMdExcludes`, no hooks |
| `.claude/tests/`, structural tests | absent | absent |
| `.claude/skills/` | `finalize`, `swarm-execute` (+ tier files), `swarm-plan`, `swarm-review` (older swarm layout) | absent |
| `.claude/agents/` | 9 `worker-*.md` project agents | absent |
| `grimoire.toml` | absent | present: bundle `typescript-essentials` and rule `css-theming` from `ghcr.io/ocx-sh/lore` |
| `.agents/` | `handover_grim_annotation_fields.md` only; tracked | `plans/` with 3 files; tracked |
| `.gitignore` for agent state | `.claude/state/`, `.claude/hooks/.state/` | `.claude/*` with negations for `rules.md`, `rules/`, `settings.json`; `.agents/worktrees/` (`.gitignore:39-49`) |
| Task runner | `taskfile.yml` | `taskfile.yml` |
| Cross-repo rules shared with grimoire | `artifact-kinds.md`, `grim-compat-markers.md` | same two |

Neither sibling carries the subsystem family, the hook set, the structural tests, the plan Status protocol rule (vscode has `plan-status.md`), the learnings store, `.agents/memory/hex.md`, or the template set. The two shared rules `artifact-kinds.md` and `grim-compat-markers.md` are the only existing cross-repo first-party pattern and track facts of grim itself.

## 15. Findings that are not obvious from the lists

1. `pre_push_main_blocker.py` is not wired in `.claude/settings.json`; `test_hooks.py` still tests it. A copy inherits an unwired hook.
2. `.claude/hooks/.state/commit-verified` is written by the root `taskfile.yml` (`.verify:mark`, `:80-85`), not by the hook folder. The hook set does not work in a repository whose `verify` task lacks that step.
3. `CONTEXT_REMINDERS` points only at `src/**` paths. A Rust plugin whose source is `src/` of its own repository matches the same rows; a Tauri repository with `src-tauri/` and `src/` (front end) matches none correctly.
4. `AGENTS.md` sits exactly at its 280 line budget; adding a repository-specific section to a copied file needs another cut.
5. `subsystem-mirror.md` invariant 1 and its `include_str!` read of grim's `src/cli/exit_code.rs` (`values_match_grims_exit_codes`) are the first tested couplings between the mirror and grim; they break at the split.
6. Skills `docs`, `changelog`, `prepare-release`, `upstream-refresh` and the `docs-style.md` rule have no meaning without the docs site, release pipeline and harness ledger.
7. The user-level `~/.claude/CLAUDE.md` rules (model routing, worktree hygiene, reporting) and the user-level hook that writes `.claude/state/subagents.jsonl` are not in the repository and need nothing copied.
