# Meta-Plan: /swarm-review max — feat/vendor-wave-expansion

## Classification

- **Tier:** max (auto, confident — ≥2 independent max signals)
- **Signals:**
  - `file_count` 56 (> 15 → max)
  - `lines_changed` 6471 (+6220 / -251)
  - `subsystems_touched` 5+ (`src/install`, `src/command`, `src/mcp`, `src/tui`, `test/`, plus `docs/`, `.claude/`, `catalog/`)
  - Structural markers: 6 new vendor renderer modules (`vendor_{cursor,gemini,junie,kiro,zed,amp}.rs`, +2339 lines), `src/install/vendor.rs` trait surface change (+71), `path_anchor.rs` (+339), `prune.rs` (+192), `client_target.rs` (+422) — canonical client value set widened from 4 → 10
  - No PR (branch target), so no label signals
- **Confidence:** confident. Gate fires because tier=max auto-fires it (cost transparency), not because of ambiguity.

## Baseline

- **Base:** `main` (default — no `--base` flag, target is a branch not a PR)
- **Target:** `feat/vendor-wave-expansion` @ `f2ad594` (worktree `~/dev/grimoire-wt-vendor-wave`)
- **Range:** `main...feat/vendor-wave-expansion`, 15 commits

> Note: `rtk` filtered `git diff --shortstat`/`--name-only` to empty output. All metrics above obtained via `rtk proxy git ...`. Workers must be told the same.

## Overlays

| Axis | Value | Source |
|---|---|---|
| breadth | `adversarial` | tier default (max) |
| reviewer | `opus` | max + adversarial breadth (`overlays.md` reviewer axis) |
| doc-reviewer | `sonnet` | 8 doc files incl. primary user guide (`docs/src/{clients,commands,concepts,configuration,agents,mcp-servers}.md`) → haiku trigger does NOT fire |
| rca | `on` (all findings above Suggest) | tier default (max) |
| codex | `on`, model `sol` | tier default (max, mandatory gate) |

## Workers

**Stage 1 — Correctness (3 parallel, opus)**
1. `worker-reviewer` spec-compliance, phase `post-implementation` — trace 6 new vendors against `adr_vendor_wave_expansion.md`, `adr_client_compat_matrix.md`, `adr_managed_context_block.md`
2. `worker-reviewer` quality / test-coverage lens — `test_render_clients.py` (+504), `test_global.py` (+309), `test_shared_skills.py` (+125) vs 2339 lines of new renderer code; shared-skill refcount guard edge cases
3. `worker-reviewer` compatibility — **critical**: canonical client set 4 → 10, `install_state.rs` schema (+24), `json_splice.rs` (+22), `path_anchor.rs` rewrite (+339). Additive-only per Principle 9; layout moves need migration + reaper + upgrade fixture

**Stage 2 — Adversarial panel (6 parallel)**
4. `worker-reviewer` quality + CLI-UX lens (opus) — `status_badge.rs`, `tui/app.rs`, `command/{status,lock,fetch}.rs`
5. `worker-reviewer` security (opus) — path traversal in `path_anchor.rs`, JSON splice into vendor config files, `prune.rs` deletion paths, env-var-driven roots
6. `worker-reviewer` performance (opus) — `prune.rs` walk, refcount scan across 10 clients
7. `worker-doc-reviewer` (sonnet) — 8 docs pages + 3 catalog skills + 3 `.claude/rules` vs actual CLI/renderer behavior
8. `worker-architect` (opus) — vendor trait boundary, dependency direction, 6-renderer duplication vs shared abstraction, ADR compliance
9. `worker-researcher` (sonnet) — SOTA: how Helm/npm/asdf handle multi-target render + managed marker blocks; upstream vendor capability claims vs `vendor-capability-watchlist.md`

**Phase 4** RCA (Five Whys, clustered) on all findings > Suggest.
**Phase 5** `codex-adversary` scope `code-diff --base main --model sol`, one-shot.

## Estimated cost

9 subagents (3 + 6), 7 on opus, 2 on sonnet, over a 6.5k-line diff + 1 Codex `sol` pass. Heavy run — comparable to a full max-tier execute.

## Not Doing

- No auto-fixes. No edits to the branch. No commits, no push.
- Read-only review; findings reported, handoff to `/swarm-execute` if actionable.
- No verdict on `feat/announce-fork` (separate branch, not this run).
