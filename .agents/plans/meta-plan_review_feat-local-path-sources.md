# Meta-Plan: /swarm-review — feat/local-path-sources

## Classification

- **Tier:** max (auto)
- **Rationale:** 42 files, +3460 / −433 lines, cross-subsystem (config, lock,
  resolve, install, skill, command, tui, oci, api + acceptance tests). New
  feature (local path sources + dev-install) touching a data model / wire
  format across ~15 consumer sites. `file_count 42 > 15` and `≥2 subsystems`
  both fire max independently → confident.
- **Confidence:** high (max by size alone; gate fires because max auto-gates,
  not because of ambiguity).

## Baseline

- **main** (default — no `--base`, no PR target).
- Target: `HEAD` (branch `feat/local-path-sources`, 11 commits ahead).

## Overlays (max defaults, no user flags)

| Axis | Value | Source |
|---|---|---|
| breadth | adversarial | tier default |
| reviewer | opus | tier default (→opus on adversarial breadth) |
| doc-reviewer | sonnet | tier default (no narrow-scope doc trigger; docs are stale-vs-code, full audit) |
| rca | on (all findings > Suggest) | tier default |
| codex | on (`sol` / gpt-5.6-sol) | tier default (mandatory at max) |

## Workers (8 = ceiling)

**Stage 1 — Correctness (2 parallel)**
- `worker-reviewer` (spec-compliance, post-implementation) — diff vs ADR
  sub-decisions + Grimoire anchors; lifecycle traceability.
- `worker-reviewer` (quality, test-coverage lens) — do `test_path_deps.py` /
  `test_dev_install.py` cover XOR wire, offline pin, repack-twice-same-hash,
  bundle provenance, dev-record prune-immunity?

**Stage 2 — Adversarial panel (6 parallel)**
- `worker-reviewer` (quality + CLI-UX lens) — command surface: `add <path>`,
  `install <path>`, status/update/uninstall.
- `worker-reviewer` (security) — path traversal, absolute-path policy,
  symlink escape, hash integrity gate, offline guarantees.
- `worker-reviewer` (performance) — per-invocation repack cost, DirWalker,
  allocations in resolve/install hot paths.
- `worker-doc-reviewer` — CLI/docs drift (docs/src/commands.md, stability.md,
  env-var table), catalog skill drift (grim-usage/grim-authoring).
- `worker-architect` — source-discriminant enum boundary, `PinnedIdentifier`
  invariant integrity, XOR wire pattern, dependency direction, ADR compliance.
- `worker-researcher` — SOTA gap vs Cargo `path=`, npm `file:`, uv/pip
  editable; SHA-256-over-canonical-tar-as-digest soundness; known pitfalls
  (absolute-path portability, no-watcher staleness).

## Pipeline

1. Discover — load subsystem rules (cli, cli-api, cli-commands,
   file-structure, tests), ADR, product-context, quality-rust/python.
2. Stage 1 (2 workers, parallel).
3. Stage 2 (6 workers, parallel).
4. RCA — Five Whys on all findings > Suggest; cluster by root.
5. Codex cross-model gate (`code-diff --base main --model sol`); graceful skip.
6. Verdict + report.

## Estimated cost

~8 Claude workers (2× spec/test + 6 adversarial, opus-heavy) + 1 Codex sol
pass + RCA synthesis. Highest-cost review tier.

## Not Doing

- No auto-fixes, no commits (review is read-only).
- No pushing to remote.
- No `--watch` scope creep review (ADR sub-decision 9 — out of scope).
