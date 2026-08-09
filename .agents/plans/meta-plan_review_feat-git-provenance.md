# Meta-Plan: /swarm-review — feat/git-provenance

## Classification

- **Tier:** max (auto)
- **Rationale:** 23 files, +1091 / −85, cross-subsystem
  (oci, catalog, command, tui, api + docs + tests). `>15 files` AND
  `≥2 subsystems` both fire the max row → confident, no tier split.
- **Confidence:** confident (two independent max signals)

## Baseline

- **Base:** `main` (default — no `--base` flag, target is branch HEAD)
- **Target:** HEAD (branch `feat/git-provenance`, commit `147a4b9`)
- **Diff:** 23 files, +1091 / −85

## Overlays (final resolved config)

| Axis | Value | Source |
|---|---|---|
| breadth | adversarial | tier default (max) |
| reviewer | opus | max + adversarial breadth |
| doc-reviewer | sonnet | broad doc scope (touches primary user guide `docs/src/publishing.md`, `commands.md`) — narrow-haiku trigger does not fire |
| rca | on (all findings > Suggest) | tier default (max) |
| codex | on (mandatory) | tier default (max); codex 1.0.3 present → will run |

## Workers per perspective

**Stage 1 — Correctness (2 parallel):**
- spec-compliance — `worker-reviewer` (spec-compliance, post-implementation), opus
- test-coverage — `worker-reviewer` (quality, lens: test-coverage), opus

**Stage 2 — Adversarial breadth (≤8 parallel):**
- quality — `worker-reviewer` (quality), opus
- security — `worker-reviewer` (security), opus — **git subprocess + remote-URL parsing = injection/validation surface**
- performance — `worker-reviewer` (performance), opus
- documentation — `worker-doc-reviewer`, sonnet
- architecture — `worker-architect` (adversarial), opus
- CLI UX — `worker-reviewer` (quality, lens: cli-ux, adversarial), opus
- SOTA — `worker-researcher` (adversarial)

**Cross-model gate:**
- `codex-adversary` (scope: code-diff `--base main`), one-shot after Claude panel converges

## Estimated cost

~9 Claude workers (opus-heavy) + 1 Codex pass. Two stages
(2 then 7 parallel), then cross-model gate, then RCA synthesis.

## Not Doing

- NO auto-fixes — review is read-only
- NO commits, NO push
- NO changes outside diff scope (`main...HEAD`)
- NO approving with unresolved Block-tier findings
