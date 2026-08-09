# Meta-Plan: /swarm-review — client-desync-tolerance

## Classification

- **Tier (auto):** borderline **high ↔ max** → low-confidence → gate fires
- **Diff metrics:** 16 files, +1119 / −116 lines
  - 15 code files (14 Rust + 1 pytest) + 1 plan markdown artifact
  - Subsystems: `src/**` (catalog, command, install, tui, error) + `test/**` → ≥2
  - Structural marker: removed `pub` variant `ConfigSync` + `config_sync` ctor (dead-code cleanup, internal — single binary crate, no lib API)
- **Confidence note:** file count (16) and ≥2 subsystems push **max**; but logic is concentrated in `installer.rs` / `status.rs` / `uninstall.rs` / `opencode_config.rs`, and most other touches are mechanical threading of one `active_outputs` helper + `active: &[ClientTarget]` param. Substantive review surface ≈ high-tier.

## Baseline

- **Base:** `main` (default — no `--base` flag, not a PR)
- **Target:** `HEAD` (branch `fix/client-desync-tolerance`, 4 commits)

## Overlays (proposed, high-tier defaults)

| Axis | Value | Source |
|---|---|---|
| breadth | full (quality / security / perf / docs) | tier default |
| reviewer | sonnet | tier default |
| rca | on (Block/High) | tier default |
| codex | off (auto-on candidate: removed pub item, correctness-critical) | tier default |

If escalated to **max**: breadth=adversarial (+architect +SOTA +CLI-UX), reviewer→opus, rca all >Suggest, codex mandatory.

## Workers per perspective

**Stage 1 — Correctness (parallel):**
- `worker-reviewer` spec-compliance (phase: post-implementation) — fix matches plan C1–C9, root cause not symptom
- `worker-reviewer` quality (lens: test-coverage) — regression tests prove each cause, guard test pins over-tolerance

**Stage 2 — full breadth (parallel):**
- `worker-reviewer` quality — SOLID/DRY, the shared `active_outputs` helper, signature threading
- `worker-reviewer` security — tolerance must not mask path-traversal/escape; only `AnchorRootAbsent` tolerated
- `worker-reviewer` performance — `detect_clients` now on read paths (status/search/tui); cost per render
- `worker-doc-reviewer` — CLI/status semantics, env var docs drift

**(max only)** + `worker-architect` (boundary/dep direction) + `worker-researcher` (SOTA) + `worker-reviewer` CLI-UX lens + `codex-adversary` (mandatory).

## Adversarial focus (this diff)

1. **Over-tolerance masking real breakage** — does `present_client_missing_file_still_flags` truly guard? Any path where an active client's genuine failure is now swallowed?
2. **Merge-on-write correctness** — `installer.rs` re-attach of other-client outputs; can it duplicate or resurrect stale outputs?
3. **Security boundary** — `TraversalAttempt` / `EscapedAnchor` MUST still surface; confirm only `AnchorRootAbsent` is the tolerated arm everywhere.
4. **Read-path detection cost** — `detect_clients` added to status/search/tui render paths.
5. **Dead-code removal** — `ConfigSync` removal fully consistent (error.rs classify arm, no dangling refs).

## Estimated cost

- high: Stage 1 (2) + Stage 2 (4) = 6 workers, no Codex
- max: + 3 workers + Codex cross-model = ~9 units + Codex

## Not Doing

- NO auto-fixing (review is read-only)
- NO commits, NO push
- NO scope beyond `main...HEAD`
