# Meta-Plan: /swarm-review feat/vscode-extension-api

## Classification

- **Tier:** max (auto, confident)
- **Diff metrics:** 68 files, +5545 / −317 lines, 8 commits in `main..HEAD`
- **Subsystems:** CLI shell + CLI API + file structure (`src/**`), acceptance
  tests (`test/**`), docs (`docs/**`), catalog (`catalog/**`)
- **Structural markers fired:**
  - Public API breakage — `grim desc` command removed, `feat!:` breaking
    commit (→ max, `--codex` on)
  - Network / registry I/O paths — `src/oci/description.rs`,
    `registry_catalog.rs`, publish push path (→ breadth full min, codex)
  - Core data-flow modules — `src/fetch.rs`, `src/error.rs` classify chain
    (→ adversarial breadth)

## Baseline

- **Base:** `main` (default — no `--base` flag, target is current branch HEAD)
- **Target:** `feat/vscode-extension-api` @ bd6f1ee (PR #33, history rewritten
  locally, not pushed)

## Overlays (resolved)

| Axis | Value | Source |
|---|---|---|
| breadth | adversarial | tier=max default |
| reviewer | **opus** | max + adversarial breadth (overlays.md reviewer axis) |
| doc-reviewer | sonnet | wide doc scope — primary user guide touched (no haiku trigger) |
| rca | on (all findings > Suggest) | tier=max default |
| codex | on (mandatory final gate) | tier=max default |
| codex-model | sol (`gpt-5.6-sol`) | tier=max default |

## Workers per perspective

| Phase | Worker | Model | Focus |
|---|---|---|---|
| Stage 1 | worker-reviewer | opus | spec-compliance (post-implementation, vs ADR) |
| Stage 1 | worker-reviewer | opus | quality, lens: test-coverage |
| Stage 2 | worker-reviewer | opus | quality + CLI-UX lens (command surface touched) |
| Stage 2 | worker-reviewer | opus | security |
| Stage 2 | worker-reviewer | opus | performance |
| Stage 2 | worker-doc-reviewer | sonnet | full trigger-matrix audit |
| Stage 2 | worker-architect | opus | SOLID, boundaries, ADR compliance |
| Stage 2 | worker-researcher | sonnet | SOTA gap (Cargo/npm/uv/Helm docs-companion patterns) |
| Final | codex-adversary | sol | code-diff --base main, one-shot |

Stage 1 (2) ∥ then Stage 2 (6) ∥ — respects 8-worker ceiling per stage.

## Estimated cost

- 8 Claude workers (6× opus, 2× sonnet) over a 5.8k-line diff + 1 Codex sol
  pass — heavyweight; expected largest spend in Stage 2 opus panel.

## Spec anchors for reviewers

- `.agents/adr/adr_description_companion.md` (accepted, amended)
- `.agents/handover_vscode_description_api.md` (frozen contract)
- Plan: description companion v2 + ErrorReason (implemented this session)

## Not Doing

- No auto-fixes — review is read-only; findings reported, not committed
- No commits, no push
- No re-run of `task verify` as part of review (already green on bd6f1ee)
