# Meta-Plan — /swarm-review (release/road-to-1.0)

## Classification

- **Tier:** max (confident)
- **Signals:** 133 files / +4069−939 / 19 commits vs `main`; ≥6 `src/`
  subsystems + tests + docs + rules + catalog; **public API breakage**
  (3 breaking `!` commits freezing the JSON contract); **new command
  surfaces** (`grim context`, `grim fetch`); network/registry I/O paths
  (fetch, oci/access). Multiple One-Way Door High markers → max.

## Baseline (decision at gate)

| Option | Base | Commits | Diff |
|---|---|---|---|
| Full release delta (default) | `main` | 19 | 133 files / +4069−939 |
| Plan work only | `7dea395` | 11 | 89 files / +2304−516 |
| Last commit | `HEAD~1` | 1 | catalog only |

`base=main` reviews everything about to land (incl. 8 pre-plan commits:
status outputs, registry login fix, scope-flag refactor, clobber guard,
exit-79 fix). Plan-only scopes to the JSON-interface work just completed.

## Overlays (max defaults)

- **breadth:** adversarial (spec-compliance + test-coverage + quality +
  security + performance + docs + architect + SOTA + CLI-UX)
- **reviewer model:** opus (max + adversarial breadth)
- **doc-reviewer:** sonnet (touches primary user guide — not narrow)
- **rca:** on (all findings above Suggest)
- **codex:** on (mandatory cross-model code-diff gate)

## Workers per perspective

- **Stage 1 (correctness):** worker-reviewer ×2 — spec-compliance
  (post-implementation), test-coverage
- **Stage 2 (adversarial):** worker-reviewer ×4 — quality, security,
  performance, CLI-UX; worker-architect (SOLID/boundary/dependency,
  adversarial); worker-doc-reviewer; worker-researcher (SOTA gap)
- **Cross-model:** codex-adversary (code-diff, one-shot)
- Max 8 concurrent; staged.

## Estimated cost

~13 worker launches (opus reviewers + opus architect) + 1 Codex pass over
a large diff. Non-trivial. This is the pre-land release gate.

## Not Doing

- No auto-fixes, no commits, no push — review is read-only.
- Findings reported (actionable / deferred), verdict at end.
