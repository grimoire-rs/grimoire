# AGENTS.md

Single project-context file for **every** AI agent working in this repo —
Claude Code, Codex CLI, and any other `AGENTS.md`-aware tool.

> **Edit this file, never `CLAUDE.md`.** `CLAUDE.md` is a one-line pointer
> that imports this one, so a second copy would only drift. The same goes
> for anything it links: fix the source file, not a restatement of it.

Lines beginning `@` are Claude Code file imports and are expanded into
context automatically; other agents should open those paths directly.

## What is Grimoire

Grimoire is a package manager for AI-agent config — a CLI to install,
maintain, and publish AI-agent configuration (skills, rules, prompts)
distributed through standard OCI registries. The binary is named `grim`;
the Rust crate/package is `grimoire`. Shipping: full CLI (20 subcommands),
OCI registry pipeline, catalog publishing, MCP server, TUI. One binary
crate, subsystem modules under `src/`, no stable *library* API — the
binary is the only consumer.

> **Status: stabilizing — preparing 1.0.0.** Released surfaces (CLI, JSON
> output, schemas, layouts, OCI pipeline, catalog publishing) are frozen
> contracts: **breaking changes are prohibited** — evolution is additive-only
> (Principle 9). Treat docs as contracts, flag drift when you find it.

## Project Identity

Product vision, target users, positioning, related repos, comparable tools
and research keywords → [`product-context.md`](./.claude/rules/product-context.md).
Consult when reasoning about project direction, scope trade-offs, ADR
motivation, or research framing. Canonical — keep current (update protocol
at the bottom of that file).

## Rule Catalog

Before planning, research, or an architectural decision, scan "By concern"
in the catalog below. Path-glob rules fire only when a matching file is
already open; the catalog covers everything before that.

@.claude/rules.md

## Build & Development Commands

**Task runner**: [`task`](https://taskfile.dev) (Taskfile v3) is the
primary runner. **Always check `task --list` before inventing ad-hoc
commands.** Taskfiles are tree-structured: root (`taskfile.yml`), subsystem
dirs (`test/`, `.claude/`), `taskfiles/*.taskfile.yml` for cross-cutting.

**Key workflows:**
```sh
task                           # fast check (format + clippy + cargo check)
task verify                    # full quality gate (lint, then build + tests)
task --force verify            # bypass caching — run everything
task rust:verify               # Rust-only gate
task shell:verify              # shell-only gate (shellcheck + shfmt)
task claude:tests              # AI config structural tests
```

**Cargo commands** (for finer control): `cargo check`, `cargo build
--release` (binary `grim`), `cargo fmt`, `cargo clippy`, `cargo test`.

**Always run `task verify` after implementation is done.** Always run
`cargo fmt` before commit. Subsystem verify tasks (`rust:verify`,
`shell:verify`, `claude:verify`) are AI dev-loop gates — run the subsystem
gate for the code changed; full `task verify` is the final gate before
commit. Conventions →
[subsystem-taskfiles.md](./.claude/rules/subsystem-taskfiles.md).

## Architecture

**Layout**: a single binary crate. All source under `src/`; binary `grim`;
crate/package `grimoire`. No workspace, no lib/CLI split. Acceptance tests
under `test/`. Rust edition 2024.

**Read the matching subsystem rule before working on code in that area** —
each carries invariants and design decisions not obvious from the code.
Claude Code auto-loads them on path match; other agents open them by hand.

| Path | Subsystem rule |
|---|---|
| `src/**` | [subsystem-file-structure.md](./.claude/rules/subsystem-file-structure.md), [subsystem-cli.md](./.claude/rules/subsystem-cli.md), [subsystem-cli-api.md](./.claude/rules/subsystem-cli-api.md), [subsystem-cli-commands.md](./.claude/rules/subsystem-cli-commands.md) |
| `test/**` (pytest, fixtures) | [subsystem-tests.md](./.claude/rules/subsystem-tests.md) |
| `.github/workflows/**` | [subsystem-ci.md](./.claude/rules/subsystem-ci.md) |
| `taskfile.yml`, `taskfiles/**` | [subsystem-taskfiles.md](./.claude/rules/subsystem-taskfiles.md) |

Beyond the path map, two rules carry weight on any change:
[`arch-principles.md`](./.claude/rules/arch-principles.md) (design
principles, boundaries, glossary) and `quality-core.md` (SOLID/DRY/YAGNI,
Block/Warn/Suggest severity tiers, refactoring discipline). Shareable,
project-independent quality guidance lives in `.claude/rules/quality-*.md`
— load the one matching the language you are editing (`quality-rust.md`,
`quality-python.md`, `quality-bash.md`), and `quality-security.md` (attack
surfaces, OWASP/STRIDE checklist) before any security review.

## Environment Variables

| Variable | Purpose | Default |
|---|---|---|
| `GRIM_HOME` | Root data directory (content store, catalog, global config, global-scope install state at `$GRIM_HOME/state/global.json`). Project-scope install state lives at `<workspace>/.grimoire/state.json`. Global-scope client output lands in vendor-native dirs — see subsystem-file-structure.md | `~/.grimoire` |
| `GRIM_DEFAULT_REGISTRY` | Default registry for short-id resolution and the single-registry fallback when no `[[registries]]` is configured. Does **not** collapse or restrict the multi-registry browse set — `[[registries]]` is authoritative for browsing even when this is set; only the `--registry` flag collapses browse to one registry. Short-id precedence: `--registry` flag > `GRIM_DEFAULT_REGISTRY` > project `default_registry` > global `default_registry` > built-in fallback `ghcr.io/grimoire-rs` (browse fallback: the public index `https://index.grimoire.rs`) | (unset) |
| `GRIM_OFFLINE` | Disable all network access (cache-only; default is always-fresh online resolution) | false |
| `GRIM_ANNOUNCE_TOKEN` | Forge API token for `grim publish --announce` (PR/MR creation, owner-id lookup); always wins over CI-conventional tokens (`GH_TOKEN`/`GITHUB_TOKEN`, `GITLAB_TOKEN`), which apply only when the CI server host matches the announce target host. Never logged. Separately, `CI_JOB_TOKEN` is checked for **presence only** on a host-matched GitLab CI: it enables a fallback git-transport credential for the announce push (value never read into grim, never used for the MR API) | (unset) |
| `DOCKER_CONFIG` | Directory holding the docker-compatible `config.json` read/written by `grim login`/`logout` (and the credential read path) | `~/.docker` |
| `OPENCODE_CONFIG` | OpenCode config file that grim edits for global-scope rule registration (vendor variable, honored read/write). When unset, grim falls back to `$XDG_CONFIG_HOME/opencode/opencode.json` (or `~/.config/opencode/opencode.json` if `XDG_CONFIG_HOME` is also unset). Config-file-only — no effect on skill/agent paths | (unset) |
| `CLAUDE_CONFIG_DIR`, `COPILOT_HOME`, `OPENCODE_CONFIG_DIR`, `CODEX_HOME`, `KIRO_HOME`, `GEMINI_CLI_HOME` | Vendor config-dir overrides (honored read-only). Global-scope installs follow them: `CLAUDE_CONFIG_DIR` replaces `~/.claude` (skills, rules, and agents) and relocates the global MCP registration file to `$CLAUDE_CONFIG_DIR/.claude.json`, `COPILOT_HOME` replaces `~/.copilot` (skills and agents; note VS Code's *embedded* Copilot CLI ignores it upstream), `OPENCODE_CONFIG_DIR` is the preferred install target over the XDG default for OpenCode skills and agents (additive — OpenCode scans both), `CODEX_HOME` replaces `~/.codex` for Codex agents and the MCP `config.toml` (Codex skills follow the cross-vendor `$HOME/.agents/skills` standard, NOT relocated by it), `KIRO_HOME` replaces `~/.kiro` **outright, no `.kiro` segment appended** (skills, `steering/` rules, `settings/mcp.json`; grim follows the Kiro CLI — the IDE ignores it upstream). **`GEMINI_CLI_HOME` is the opposite shape**: it replaces the *home directory*, so Gemini's root is `$GEMINI_CLI_HOME/.gemini` — segment still appended — relocating Gemini agents and `settings.json`, but never the shared `$HOME/.agents/skills` pool. They also drive global-scope client *detection* — a client counts as present when its (possibly overridden) native root exists. Details → subsystem-file-structure.md | (unset) |

## First-Party Catalog

`catalog/` holds grim-publishable packages (skills `grim-usage`,
`ai-config-authoring`, `grim-authoring`, bundle `grim-essentials`, mcp `grim`).
**CLI (`src/command/**`) or docs-page changes require a drift review of
these skills** — duty + procedure: [catalog/README.md](./catalog/README.md).
Hooks remind on matching edits; `task catalog:verify` gates CI.

## Core Principles

Nine principles distill every rule, skill, and standard in the framework.
Follow them and everything else follows.

### 1. Understand First

Read before write. Grep before create. Never modify code not read — before
changing a function, grep all callers. Check what exists before building
new.

### 2. Prove It Works

Write tests for the use case first. Run them before commit. Every bug fix
gets a regression test. All quality gates must pass.

### 3. Keep It Safe

No secrets in code. Validate all external input. Least privilege
everywhere. Flag vulnerabilities immediately.

### 4. Keep It Simple

Small functions, single responsibility. No premature abstraction. Delete
dead code. Comments explain *why*, never *what*.

### 5. Don't Repeat Yourself

Check `.claude/skills/` before ad-hoc generation. Follow existing patterns.
Single source of truth for logic. Extract only when duplication is real.

### 6. Ship It

Work on a branch, never main. Commit iteratively. **Never push to remote**
— the human decides when to push. Push triggers CI, real cost.

### 7. Leave a Trail

Planning artifacts go under `./.agents/`. Document architectural
decisions in ADRs. Name things so the next person understands.

### 8. Learn and Adapt

When you get user feedback or corrections, evaluate whether the insight
should persist as an AI config update (rules, skills, agents).

### 9. Preserve Compatibility

Stabilization freeze on the road to 1.0.0: breaking changes are prohibited — do not propose, implement, or merge one.
Treat every schema, layout, and renderer change with high caution.
Schema evolution is additive-only (new fields optional + default; enum literals added, never removed).
Layout moves ship automatic state migration, an old-path reaper, and an upgrade fixture; renderer changes prove self-heal (re-materialize leaves `status` not-modified). Contract: `docs/src/stability.md`, `adr_render_layout_stability.md`.

## Tech Stack

Golden-path technology choices — no deviations →
[`product-tech-strategy.md`](./.claude/rules/product-tech-strategy.md).
(A frontmatter-less rule: Claude Code already loads it every session, so it
is linked here rather than `@`-imported a second time.)

## Workflow

**Worktrees**: `grimoire` is the primary checkout (`main`). Feature work
happens in ad hoc worktrees — human ones as siblings `../grimoire-wt-<topic>`
on a matching `<type>/<topic>` branch (e.g. `../grimoire-wt-status-check` on
`feat/status-check`), agent-spawned ones under
`.agents/worktrees/<topic>` (gitignored). Created and torn down per task,
not a fixed roster; whoever creates one removes it. `git worktree list`
shows what is currently checked out.

**Commits**: Use [Conventional Commits](https://www.conventionalcommits.org/)
(e.g., `feat:`, `fix:`, `refactor:`, `ci:`, `chore:`). Scopes optional. No
`Co-Authored-By` trailers. Use `chore:` for AI settings, skills, agent
context files, and tooling that should not appear in the changelog.

**Landing a feature**: When a feature is done, run `/finalize` to clean
branch history into a sequence of Conventional Commits ready to
fast-forward onto `main`. Two-phase model (`/commit` during dev,
`/finalize` before landing) →
[workflow-git.md](./.claude/rules/workflow-git.md).

**Planning flow**: ADR → Design Spec → Plan → Implementation. Planning docs
are committed under `./.agents/`: `adr/`, `specs/`, `plans/` (incl.
`bugfix_plan_*`, `meta-plan_*`; landed pre-protocol ones in
`plans/archive/`), `research/`, one-off records at the root. Templates in
`./.claude/templates/artifacts/`.

## Skills & Personas

Multi-agent orchestration is the **hex** bundle, installed at user level
(not in this repo): `/hex-plan`, `/hex-execute`, `/hex-review`,
`/hex-architect`, `/hex-init`. They read swarm memory from
`.agents/memory/hex.md` — pointers, the model matrix, always-on review
perspectives, and the cross-model adversary.

<!-- hex:start -->
Swarm memory: `.agents/memory/hex.md` (pointers + preferences). Commands: `/hex-init`, `/hex-plan`, `/hex-execute`, `/hex-review`, `/hex-architect`.
<!-- hex:end -->

Project-local skills in `.claude/skills/` cover solo work and this repo's
own conventions: `/builder`, `/qa-engineer`, `/security-auditor`,
`/code-check`, `/bugfix`, `/docs`, `/commit`, `/finalize`, `/next`,
`/meta-maintain-config`, `/meta-validate-context`. Full map → "Skills by
task topic" in [.claude/rules.md](./.claude/rules.md). Check
`.claude/skills/` before ad-hoc generation.

## Starting Work

Every task starts with
[workflow-intent.md](./.claude/rules/workflow-intent.md) — classify work
(feature, bug fix, refactoring), check GitHub for related issues/PRs, then
follow the appropriate workflow. Also:
[workflow-feature.md](./.claude/rules/workflow-feature.md),
[workflow-bugfix.md](./.claude/rules/workflow-bugfix.md),
[workflow-refactor.md](./.claude/rules/workflow-refactor.md).

## Adversarial Review Guidance

When a cross-model reviewer is delegated an adversarial pass (hex's
adversary gate, or a direct hand-off to another model):

1. **Load this file and the subsystem rules** for the touched paths before
   flagging anything — the author was likely working against them.
2. **Challenge design choices**, not style. `cargo fmt` handles
   formatting; question whether the approach is right, what assumptions it
   depends on, and where it fails under real-world conditions.
3. **Do not critique load-bearing conventions** stated here or in
   `product-tech-strategy.md` (Tokio, Rust 2024, OCI-backed storage,
   never-push-to-remote, commit format). They are decisions, not
   invitations.
4. Return findings with **concrete file paths and line numbers**, grouped
   by severity (Block / Warn / Suggest — definitions in
   `.claude/rules/quality-core.md`).

---

# General Agent Safety (applies to any external agent)

## Non-interactive shell commands

`cp`, `mv`, `rm` may be aliased to `-i` on some systems, which hangs an
agent waiting for y/n. Always use non-interactive flags:

```bash
cp -f source dest           # NOT: cp source dest
mv -f source dest           # NOT: mv source dest
rm -f file                  # NOT: rm file
rm -rf directory            # NOT: rm -r directory
```

Also: `scp -o BatchMode=yes`, `ssh -o BatchMode=yes`, `apt-get -y`,
`HOMEBREW_NO_AUTO_UPDATE=1 brew …`.

## Session hygiene

- **Never push to remote** — the human decides when to push (CI has real cost).
- All changes must be committed locally on a feature branch.
- **Never commit directly to `main`**.
- Run `task verify` after any implementation change.
