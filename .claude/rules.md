# Grimoire Rule Catalog

Entry point for `.claude/rules/`. Path-scoped rules fire only on file edit match — not enough during planning, research, or architecture before code exists.

## When to consult this catalog

- Plan/architecture phase — scan "By concern" before drafting
- Research phase — find quality criteria for topic
- Onboarding new area — scan "By subsystem" / "By language"
- Writing ADRs, RFCs, research artifacts — find relevant rules
- Updating AI config — see "AI config changes" row in "By concern"

Editing code + relevant rule auto-loaded via `paths:` glob → no need re-read from catalog. Catalog for cases where path-scoping cannot fire.

## How to update this catalog

Any change to `.claude/rules/` must reflect here in same commit:

- New rule → add row in relevant tables below
- Deleted rule → remove all references
- Changed `paths:` glob → update "By auto-load path" table
- New skill → add entry in "Skills by task topic"

Structural tests in `.claude/tests/test_ai_config.py` fail when catalog drifts from reality. Run `task claude:tests` locally before committing catalog changes.

## By concern — "if you care about X, consult these"

| Concern | Rules & skills |
|---|---|
| Product vision / positioning | [product-context.md](./rules/product-context.md) — canonical identity doc; update when positioning shifts (see Update Protocol section at bottom) |
| Adopting a third-party library or tool | [product-tech-strategy.md](./rules/product-tech-strategy.md), `quality-{lang}.md` |
| Starting any task (work-type routing) | [workflow-intent.md](./rules/workflow-intent.md) (global) — classifies work, checks GitHub context, routes to correct workflow |
| Designing a new feature | [arch-principles.md](./rules/arch-principles.md), [workflow-feature.md](./rules/workflow-feature.md), `subsystem-{target}.md`, `/hex-architect` |
| Fixing a bug | [workflow-bugfix.md](./rules/workflow-bugfix.md) — Reproduce → RCA → Regression Test → Fix → Verify; skill `bugfix` (guided, enforces failing-test-first gate); skill `rca` (read-only verdict on a ticket), skill `fix` (RCA, then an approved fix plan) |
| Refactoring code | [workflow-refactor.md](./rules/workflow-refactor.md) — Safety Net → Scope → Transform → Verify → Repeat |
| Documentation work | [docs-style.md](./rules/docs-style.md) (Grimoire voice), [docs-quality.md](./rules/docs-quality.md) (vendored — page declarations, the 18 blocking rules, runnable checks under `rules/docs-quality/checks/`), skill `docs` |
| Security-sensitive change | [adr_artifact_trust_model.md](../.agents/adr/adr_artifact_trust_model.md) **first** — the trust boundary and what is out of scope; then [quality-security.md](./rules/quality-security.md), [subsystem-ci.md](./rules/subsystem-ci.md), skill `security-auditor` |
| Deciding whether a security finding is a defect or a non-goal | [arch-threat-model.md](./rules/arch-threat-model.md) — attackers T1–T5 are in scope, N1–N5 are explicit non-goals (notably **insiders with commit access**); invariants I1–I6, and the recorded hook exception under **I2** ([adr_hook_workspace_consent.md](../.agents/adr/adr_hook_workspace_consent.md)), which the `settings` kind shares ([adr_settings_artifact_kind.md](../.agents/adr/adr_settings_artifact_kind.md)) — re-deriving a decided trade as a fresh Block is the churn that file exists to stop |
| CLI command changes | [subsystem-cli.md](./rules/subsystem-cli.md), [subsystem-cli-api.md](./rules/subsystem-cli-api.md), [subsystem-cli-commands.md](./rules/subsystem-cli-commands.md) |
| Vendor renderer declines / upstream capability gaps | [vendor-capability-watchlist.md](./rules/vendor-capability-watchlist.md) — re-verify upstream before patching a decline; the date-stamped rows live in `.agents/upstream-watchlist.md` (outside `.claude/rules/`, which loads every `.md`) |
| Writing tests | [subsystem-tests.md](./rules/subsystem-tests.md), [quality-python.md](./rules/quality-python.md), [quality-rust.md](./rules/quality-rust.md), skill `qa-engineer` |
| CI / workflows | [subsystem-ci.md](./rules/subsystem-ci.md), [workflow-release.md](./rules/workflow-release.md) |
| AI config changes | [meta-ai-config.md](./rules/meta-ai-config.md) + this catalog, skill `meta-maintain-config` |
| GitHub issues & PRs / planning artifacts | [workflow-github.md](./rules/workflow-github.md), [workflow-feature.md](./rules/workflow-feature.md) |
| Commits, branches, rebasing, landing on main | [workflow-git.md](./rules/workflow-git.md), skills `commit`, `finalize` |
| Plan progress tracking (Status block + `.claude/state/current_plan.md`) | [meta-ai-config.md](./rules/meta-ai-config.md) "Plan Status Protocol", `/hex-plan`, `/hex-execute`, `/hex-review`, skills `commit`, `finalize`, `next` |
| Multi-agent orchestration | [workflow-feature.md](./rules/workflow-feature.md) — the **hex** bundle (`/hex-plan`, `/hex-execute`, `/hex-review`, `/hex-architect`), installed at project scope via the root `grimoire.toml`; swarm memory `.agents/memory/hex.md` |
| Code quality audit | [quality-core.md](./rules/quality-core.md), `quality-{lang}.md`, skill `code-check` |
| Source comments, doc comments, help/schema text | [code-docs.md](./rules/code-docs.md) (vendored), skill `code-docs-cleanup` for bulk shortening |
| Error type design (Rust) | [quality-rust.md](./rules/quality-rust.md), [quality-rust-errors.md](./rules/quality-rust-errors.md) |
| CLI exit code design (Rust) | [quality-rust.md](./rules/quality-rust.md), [quality-rust-exit_codes.md](./rules/quality-rust-exit_codes.md) |
| Taskfiles / build pipeline / caching | [subsystem-taskfiles.md](./rules/subsystem-taskfiles.md) |
| Releases | [workflow-release.md](./rules/workflow-release.md) |

## By language

| Language | Quality rule | Related |
|---|---|---|
| Rust | [quality-rust.md](./rules/quality-rust.md) | [quality-rust-errors.md](./rules/quality-rust-errors.md), [quality-rust-exit_codes.md](./rules/quality-rust-exit_codes.md), [arch-principles.md](./rules/arch-principles.md), [quality-core.md](./rules/quality-core.md) |
| Python (acceptance tests) | [quality-python.md](./rules/quality-python.md) | [subsystem-tests.md](./rules/subsystem-tests.md) |
| Bash (tasks, hooks) | [quality-bash.md](./rules/quality-bash.md) | — |
| TypeScript | [typescript-quality.md](./rules/typescript-quality.md) (vendored) | [typescript-packaging.md](./rules/typescript-packaging.md) (vendored) — no TypeScript in this repo yet; both fire if that changes |

Vendored language rules co-fire with the first-party ones on the same globs:
[rust-quality.md](./rules/rust-quality.md) and [rust-cargo.md](./rules/rust-cargo.md) alongside
`quality-rust*.md`, [python-quality.md](./rules/python-quality.md) and
[python-packaging.md](./rules/python-packaging.md) alongside `quality-python.md`. See
"Vendored rules" below — the overlap is real and unresolved.

## By subsystem

Catalog = single source of truth — `AGENTS.md` may summarize or reference by pointer.

| Subsystem | Rule | Path scope |
|---|---|---|
| Storage / file structure (core and index) | [subsystem-file-structure.md](./rules/subsystem-file-structure.md) | `src/**`, `crates/**` |
| Install layout: skills, rules | [subsystem-install-layout-skills-rules.md](./rules/subsystem-install-layout-skills-rules.md) | `src/install/{materializer,expected_outputs,render,installer}.rs` |
| Install layout: MCP servers, agents | [subsystem-install-layout-agents-mcp.md](./rules/subsystem-install-layout-agents-mcp.md) | `src/install/{mcp_config,json_splice,toml_splice,cline_lock,vendor_agents}.rs` |
| Install layout: hooks | [subsystem-install-layout-hooks.md](./rules/subsystem-install-layout-hooks.md) | `src/hook/**`, `src/install/hook_*.rs`, `src/install/launcher.rs`, `crates/grim-hook/**`, `crates/grimoire-hook-core/**` |
| Install layout: settings | [subsystem-install-layout-settings.md](./rules/subsystem-install-layout-settings.md) | `src/install/settings_*.rs` |
| Vendor paths (global-scope roots, env overrides) | [subsystem-vendor-paths.md](./rules/subsystem-vendor-paths.md) | `src/install/vendor.rs`, `src/install/vendor_*.rs`, `src/install/{prune,claude_config,opencode_config,managed_config}.rs` |
| Client detection | [subsystem-client-detection.md](./rules/subsystem-client-detection.md) | `src/install/target.rs`, `src/install/client_target.rs` |
| Install state and path containment | [subsystem-install-state.md](./rules/subsystem-install-state.md) | `src/install/install_state.rs`, `src/path_safety.rs` |
| Path anchors | [subsystem-path-anchors.md](./rules/subsystem-path-anchors.md) | `src/install/path_anchor.rs` |
| CLI shell | [subsystem-cli.md](./rules/subsystem-cli.md) | `src/**`, `crates/**` |
| CLI API / output formats | [subsystem-cli-api.md](./rules/subsystem-cli-api.md) | `src/**`, `crates/**` |
| CLI command index | [subsystem-cli-commands.md](./rules/subsystem-cli-commands.md) | `src/**`, `crates/**` |
| Config key title/description style | [subsystem-config-keys.md](./rules/subsystem-config-keys.md) | `src/command/config_keys.rs`, `src/config/declaration.rs` |
| Acceptance tests | [subsystem-tests.md](./rules/subsystem-tests.md) | `test/**` |
| CI / workflows | [subsystem-ci.md](./rules/subsystem-ci.md) | `.github/workflows/**` |
| Taskfiles | [subsystem-taskfiles.md](./rules/subsystem-taskfiles.md) | `taskfile.yml`, `taskfiles/**/*.yml`, `**/taskfile.yml` |

The four `src/**`+`crates/**`-scoped subsystem rules (`subsystem-cli.md`,
`subsystem-cli-api.md`, `subsystem-cli-commands.md`,
`subsystem-file-structure.md`) co-fire while the workspace's crates are
provisional; the coupling is intended (declared below).
`subsystem-file-structure.md` is the core and index of the file-structure family;
the eight parts above carry narrower globs, so each loads only with the code it
describes. One part shares a glob string: `subsystem-vendor-paths.md` and
`vendor-capability-watchlist.md` both list `src/install/vendor_*.rs`, a declared
group below. No other part needs one.

## By auto-load path — "what fires when you edit"

| Edit path | Rules that auto-load |
|---|---|
| `**/*.rs` | [quality-rust.md](./rules/quality-rust.md), [quality-rust-errors.md](./rules/quality-rust-errors.md), [quality-rust-exit_codes.md](./rules/quality-rust-exit_codes.md) [rust-quality.md](./rules/rust-quality.md) (vendored) (+ [arch-principles.md](./rules/arch-principles.md) under `src/**`, `crates/**`, `external/**`) |
| `**/Cargo.toml`, `**/Cargo.lock` | [quality-rust.md](./rules/quality-rust.md) (+ [rust-cargo.md](./rules/rust-cargo.md) on `**/Cargo.toml`, `**/clippy.toml`, `**/rustfmt.toml`, `**/deny.toml`, `**/rust-toolchain.toml`) |
| `src/**`, `crates/**` | + [subsystem-cli.md](./rules/subsystem-cli.md), [subsystem-cli-api.md](./rules/subsystem-cli-api.md), [subsystem-cli-commands.md](./rules/subsystem-cli-commands.md), [subsystem-file-structure.md](./rules/subsystem-file-structure.md) |
| `src/hook/**`, `src/install/hook_*.rs`, `src/install/launcher.rs`, `crates/grim-hook/**`, `crates/grimoire-hook-core/**` | + [subsystem-install-layout-hooks.md](./rules/subsystem-install-layout-hooks.md) |
| `src/install/{materializer,expected_outputs,render,installer}.rs` | + [subsystem-install-layout-skills-rules.md](./rules/subsystem-install-layout-skills-rules.md) |
| `src/install/{mcp_config,json_splice,toml_splice,cline_lock,vendor_agents}.rs` | + [subsystem-install-layout-agents-mcp.md](./rules/subsystem-install-layout-agents-mcp.md) |
| `src/install/settings_*.rs` | + [subsystem-install-layout-settings.md](./rules/subsystem-install-layout-settings.md) |
| `src/install/vendor.rs`, `src/install/vendor_*.rs`, `src/install/{prune,claude_config,opencode_config,managed_config}.rs` | + [subsystem-vendor-paths.md](./rules/subsystem-vendor-paths.md) |
| `src/install/target.rs`, `src/install/client_target.rs` | + [subsystem-client-detection.md](./rules/subsystem-client-detection.md) |
| `src/install/install_state.rs`, `src/path_safety.rs` | + [subsystem-install-state.md](./rules/subsystem-install-state.md) |
| `src/install/path_anchor.rs` | + [subsystem-path-anchors.md](./rules/subsystem-path-anchors.md) |
| `src/install/vendor_*.rs`, `src/oci/mcp.rs`, `src/catalog/rating_provider.rs`, `src/install/settings_phase.rs` | + [vendor-capability-watchlist.md](./rules/vendor-capability-watchlist.md) |
| `src/oci/**`, `src/install/**`, `src/command/hook*`, `src/command/{login,logout}.rs`, `src/command/{publish,release}*`, `catalog/**`, `crates/**` | + [arch-threat-model.md](./rules/arch-threat-model.md) |
| `src/command/config_keys.rs`, `src/config/declaration.rs` | + [subsystem-config-keys.md](./rules/subsystem-config-keys.md) |
| `test/**` | [subsystem-tests.md](./rules/subsystem-tests.md) |
| `test/**/*.py`, `**/*.py` | + [quality-python.md](./rules/quality-python.md), [python-quality.md](./rules/python-quality.md) (vendored) |
| `**/pyproject.toml`, `**/uv.lock` | [python-packaging.md](./rules/python-packaging.md) (vendored) |
| `README.md`, `CHANGELOG.md`, `CONTRIBUTING.md`, `docs/**` and other generators' roots | [docs-quality.md](./rules/docs-quality.md) (vendored) |
| `docs/**` | + [docs-style.md](./rules/docs-style.md) |
| `docs/src/content/docs/ratings.md`, `docs/src/content/docs/settings.md` | + [vendor-capability-watchlist.md](./rules/vendor-capability-watchlist.md) (each carries its feature's own upstream claims: forge versions, harness settings limits) |
| `**/*.sh`, `**/*.bash` | [quality-bash.md](./rules/quality-bash.md) |
| `**/*.ts`, `**/*.tsx`, `**/*.mts`, `**/*.cts` | [typescript-quality.md](./rules/typescript-quality.md) (vendored) |
| `**/package.json`, `**/tsconfig*.json`, `**/eslint.config.*`, `**/biome.json*` | [typescript-packaging.md](./rules/typescript-packaging.md) (vendored) |
| `.github/workflows/**`, `.github/actions/**`, `.github/dependabot.yml` | [subsystem-ci.md](./rules/subsystem-ci.md), [quality-security.md](./rules/quality-security.md) |
| `.github/ISSUE_TEMPLATE/**` | [workflow-github.md](./rules/workflow-github.md) |
| `dist-workspace.toml`, `cliff.toml`, `CHANGELOG.md`, release workflows | [workflow-release.md](./rules/workflow-release.md), [workflow-git.md](./rules/workflow-git.md) |
| `taskfile.yml`, `taskfiles/**/*.yml`, `**/taskfile.yml` | [subsystem-taskfiles.md](./rules/subsystem-taskfiles.md) |
| `.agents/plans/**`, `.agents/adr/**`, `.agents/specs/**`, `.agents/research/**` | [product-context.md](./rules/product-context.md), [workflow-feature.md](./rules/workflow-feature.md) |
| `.claude/**` | [meta-ai-config.md](./rules/meta-ai-config.md) |

Globals (always loaded or imported into `AGENTS.md`): [quality-core.md](./rules/quality-core.md),
[product-tech-strategy.md](./rules/product-tech-strategy.md), [workflow-intent.md](./rules/workflow-intent.md),
[hex-state.md](./rules/hex-state.md) (vendored), this catalog. Strict definition of the set: `meta-ai-config.md` › "Current Global Rules".

Scoped workflow rules (loaded by path match, consumed by skills on demand):
[quality-security.md](./rules/quality-security.md) (`.github/workflows/**`, `.github/actions/**`),
[workflow-github.md](./rules/workflow-github.md) (`.github/ISSUE_TEMPLATE/**`),
[workflow-git.md](./rules/workflow-git.md) (`CHANGELOG.md`, `cliff.toml`, `dist-workspace.toml`).

## Declared Path-Scope Overlaps

Every pair of rules sharing `paths:` pattern must be covered by declared group. Undeclared overlaps fail `test_path_overlaps_declared_or_absent`.

Exempt from overlap detection (intended broad coupling):

- Shareable `quality-*.md` — language quality rules co-fire with subsystem rules on language globs.
- `workflow-bugfix.md` / `workflow-refactor.md` — source-work-surface scope per `adr_ai_config_path_scope_correction.md`; co-firing with subsystem rules = intended coupling.

| Declared group | Shared scope |
|---|---|
| `quality-security.md` + `subsystem-ci.md` | `.github/workflows/**`, `.github/actions/**` |
| `workflow-git.md` + `workflow-release.md` | `CHANGELOG.md`, `cliff.toml`, `dist-workspace.toml` |
| `product-context.md` + `workflow-feature.md` | `.agents/plans/**`, `.agents/adr/**`, `.agents/specs/**`, `.agents/research/**` |
| `docs-style.md` + `product-context.md` | `docs/**` |
| `docs-quality.md` + `docs-style.md` + `product-context.md` | `docs/**` (vendored rule co-fires with both) |
| `docs-quality.md` + `workflow-git.md` + `workflow-release.md` | `CHANGELOG.md` (vendored rule carries changelog conventions; the other two carry the release process) |
| `subsystem-cli.md` + `subsystem-cli-api.md` + `subsystem-cli-commands.md` + `subsystem-file-structure.md` | `src/**`, `crates/**` (cargo workspace, provisional crate split) |
| `arch-threat-model.md` + `arch-principles.md` + the four `src/**` subsystem rules | `src/oci/**`, `src/install/**`, `src/command/hook*`, `src/command/{login,logout,publish,release}*`, `crates/**` — the security-sensitive subset of `src/**` plus the hook crates (the native launcher runs on every client hook event); the threat model co-fires with them by design, since a boundary question is always also a design question. `src/command/hook*` joined when the dispatcher runtime landed: it executes code a client runs automatically, which is the sharpest edge of the boundary |
| `code-docs.md` + `rust-quality.md` / `python-quality.md` / `typescript-quality.md` | `**/*.rs`, `**/*.py`, `**/*.pyi`, `**/*.ts`, `**/*.tsx`, `**/*.mts`, `**/*.cts` (vendored comment rule co-fires with each language set; it owns what a comment keeps, they own syntax) |
| `arch-threat-model.md` + `subsystem-cli.md` + `subsystem-cli-api.md` + `subsystem-cli-commands.md` + `subsystem-file-structure.md` | `crates/**` (hook crates: the launcher is security-sensitive and shares the subsystem scope) |
| `subsystem-vendor-paths.md` + `vendor-capability-watchlist.md` | `src/install/vendor_*.rs` (the global-scope roots and the upstream re-verify procedure both govern a vendor renderer edit) |
| `arch-threat-model.md` + `quality-security.md` | `catalog/**` (published artifacts — the boundary scopes the checklist) |

## Vendored rules — installed, not authored here

Nine rules under `.claude/rules/` come from upstream bundles: their prose is **not authored here**, and a local edit to it is reverted by the next sync. (Two local edits exist, both lost on sync and re-applied after every one: a shebang on three shell fixtures under `rules/docs-quality/checks/`, which `task shell:verify` requires; and the Starlight branch of `rules/docs-quality/checks/nav_depth.py` plus its `fixtures/nav_depth/pass-nav-starlight/` fixture, which the docs-site tests require — upstream `ocx-sh/grimoire-lore` never had it.) They declare provenance in frontmatter (`license:`, `repository:`), which is the marker `.claude/tests/test_ai_config.py::is_vendored` uses to exempt them from this repo's *authoring* standards (dead-glob detection, catalog authorship, skill description and body budgets). Fix upstream, then re-sync. The same holds for the 15 vendored skills under `.claude/skills/` (`code-docs-cleanup`, `docs-instrument`, `docs-plan`, `docs-review`, `hex-*`, `nox-review`).

| Rule | Upstream |
|---|---|
| [docs-quality.md](./rules/docs-quality.md) (+ `rules/docs-quality/` checks and fixtures) | `ocx-sh/grimoire-lore` |
| [rust-quality.md](./rules/rust-quality.md), [rust-cargo.md](./rules/rust-cargo.md) | `ocx-sh/grimoire-lore` |
| [python-quality.md](./rules/python-quality.md), [python-packaging.md](./rules/python-packaging.md) | `ocx-sh/grimoire-lore` |
| [typescript-quality.md](./rules/typescript-quality.md), [typescript-packaging.md](./rules/typescript-packaging.md) | `ocx-sh/grimoire-lore` |
| [code-docs.md](./rules/code-docs.md) (+ `rules/code-docs/` references and checks) | `ocx-sh/grimoire-lore` |
| [hex-state.md](./rules/hex-state.md) (global) | `michael-herwig/arcana` |

**Open**: the vendored `rust-quality.md` / `python-quality.md` duplicate the first-party `quality-rust.md` / `quality-python.md` on the same globs — two rule sets now load for the same edit. Which set is authoritative is a decision for the owner, not for a sync.

## Skills by task topic

| Task topic | Skill |
|---|---|
| Writing docs | `docs` |
| Security review | `security-auditor` |
| Architecture decision | `/hex-architect` (hex bundle) |
| Code quality audit | `code-check` |
| Shortening source comments in bulk | `code-docs-cleanup` |
| Implementation / debugging | `builder` |
| Test strategy | `qa-engineer` |
| Fixing a bug (guided, failing-test-first) | `bugfix` |
| Root cause analysis of a ticket (read-only) | `rca` |
| Ticket to approved fix plan (RCA, then regression-test-first plan) | `fix` |
| Planning a feature (multi-agent) | `/hex-plan` (hex bundle) |
| Executing a feature (multi-agent) | `/hex-execute` (hex bundle) |
| Adversarial review | `/hex-review` (hex bundle; cross-model adversary configured in `.agents/memory/hex.md`) |
| Releases | (see [workflow-release.md](./rules/workflow-release.md)) |
| AI config maintenance | `meta-maintain-config`, `meta-validate-context` |
| Commits (working phase) | `commit` |
| Finalize branch for merge onto main | `finalize` |
| Site changelog page for a release | `changelog` |
| Suggest next slash command from current state | `next` |
| Upstream freshness sweep | `upstream-refresh` |
