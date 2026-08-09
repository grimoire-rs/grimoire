# Meta-Plan: /swarm-review — feat/grim-publish

## Classification

- **Tier:** max (auto, confident)
- **Rationale:** 25 files (>15 → max); ≥2 subsystems (src/**, test/**,
  `.github/workflows/**`, taskfiles, docs); structural markers: CI workflow
  change (security review required), OCI registry I/O path (`--codex`
  trigger), new public CLI surface + public manifest schema (`publish.toml`
  becomes public API)
- **Diff metrics:** 25 files, +3658 / −161 vs `main`

## Baseline

- **Base:** `main` (default — no `--base` flag, no PR target)
- **Target:** HEAD (branch `feat/grim-publish`, 4 commits: c451917, 0e848d8,
  88ec610, d38b2a1)

## Overlays (resolved)

| Axis | Value | Source |
|---|---|---|
| breadth | adversarial | tier=max default |
| reviewer | opus | max + adversarial breadth escalation (overlays.md) |
| doc-reviewer | sonnet | diff touches primary user-guide pages (`publishing.md`, `commands.md`) — narrow haiku trigger does not fire |
| rca | on (all findings > Suggest) | tier=max default |
| codex | on | tier=max mandatory final gate |

**Cost note:** Codex code-diff pass already ran once during
`/swarm-execute` (1 actionable auto-fixed: channel-tag move). Max-tier
review re-runs it read-only as mandatory gate — second pass sees the
post-fix diff, so not pure duplication, but findings overlap likely.

## Workers per perspective

| Phase | Workers | Model |
|---|---|---|
| Stage 1 — Correctness | `worker-reviewer` spec-compliance (post-implementation) + `worker-reviewer` quality/test-coverage | opus |
| Stage 2 — Adversarial | `worker-reviewer` quality+CLI-UX, security, performance (3) | opus |
| Stage 2 — Adversarial | `worker-doc-reviewer` | sonnet |
| Stage 2 — Adversarial | `worker-architect` (ADR D1–D7 compliance, boundaries) | opus |
| Stage 2 — Adversarial | `worker-researcher` (SOTA: helm, cargo-release, npm, melos batch-publish semantics) | sonnet |
| Cross-model | `codex-adversary` code-diff `--base main` | — |

Stage 1 (2) + Stage 2 (6) = 8 workers — at concurrency ceiling.

## Estimated cost

- 8 review workers (5 opus, 3 sonnet-class) + 1 Codex pass + RCA synthesis
- Heaviest review tier — diff is one-way-door (public manifest schema,
  CI publishing path)

## Not Doing

- NO auto-fixes — review is read-only; actionable findings reported for
  `/swarm-execute` handoff
- NO commits, NO push
- NO re-entering Review-Fix Loop (already converged in /swarm-execute)
