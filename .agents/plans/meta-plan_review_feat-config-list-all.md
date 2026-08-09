# Meta-Plan: /swarm-review feat/config-list-all

## Classification

- **Tier**: `max` (auto)
- **Rationale**: 19 files > 15-file max threshold; 2093 lines changed (+1762/−331); ≥3 subsystems (src CLI group, acceptance tests, docs, catalog, .claude rules)
- **Confidence**: confident — multiple max signals, no adjacent-tier split
- **Structural markers**: none from One-Way Door list (no new top-level src dir, no Cargo.toml deps, no auth/network paths, no public-API breakage — single binary crate, no stable lib API)
- **Diff snapshot**: `main...HEAD`, 7 commits (feat typed key registry + `config list --all`, 2 refactors ResolvedOptions/typed defaults, feat clients string-set, 2 docs, 1 catalog)

## Baseline

- **Base**: `main` (default — no `--base` flag, target is branch HEAD)
- **Target**: `HEAD` (branch: `feat/config-list-all`)

## Overlays (resolved)

| Axis | Value | Source |
|---|---|---|
| breadth | adversarial | tier default (max) |
| reviewer | **opus** | max + adversarial breadth (overlays.md reviewer axis) |
| doc-reviewer | sonnet | default (diff touches commands.md — core reference page, narrow-doc haiku trigger not safely met) |
| rca | on (all findings > Suggest) | tier default (max) |
| codex | on (mandatory final gate) | tier default (max) |
| codex-model | sol (`gpt-5.6-sol`) | tier default (max) |

## Workers

| Phase | Worker | Model | Focus |
|---|---|---|---|
| Stage 1 (2 parallel) | worker-reviewer | opus | spec-compliance, post-implementation (vs plan artifact melodic-stirring-curry + frozen interfaces I1–I3) |
| Stage 1 | worker-reviewer | opus | quality, lens: test-coverage |
| Stage 2 (6 parallel) | worker-reviewer | opus | quality + CLI-UX lens (command surface touched) |
| Stage 2 | worker-reviewer | opus | security |
| Stage 2 | worker-reviewer | opus | performance |
| Stage 2 | worker-doc-reviewer | sonnet | full trigger-matrix audit |
| Stage 2 | worker-architect | opus | SOLID, subsystem boundaries, ADR compliance |
| Stage 2 | worker-researcher | sonnet | SOTA gap: config introspection/metadata in cargo, npm, git config, VS Code settings schema |
| Phase 5 | codex-adversary | sol | code-diff --base main, one-shot |

8 concurrent worker ceiling respected (Stage 1 and Stage 2 sequential phases).

## Estimated cost

~8 agent runs (5 opus, 2–3 sonnet) + 1 Codex pass ≈ 500–800k tokens. Context: diff already passed 2 dev-time Opus adversarial reviews + 2 gate agents during build; this is the formal full-panel pass.

## Not Doing

- No auto-fixes — review read-only; actionable findings reported, not committed
- No commits, no push
- No re-running task verify (last full gate green pre-commit; reviewers verify via diff + targeted checks)

## Cost lever (optional)

Standing session directive prefers Sonnet-heavy / Opus-rare. Skill default at max+adversarial is reviewer=opus. To halve cost: approve with note "reviewer=sonnet" — classifier metrics stay max, only reviewer axis drops.
