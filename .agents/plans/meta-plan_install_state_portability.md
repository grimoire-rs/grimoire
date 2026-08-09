# Meta-Plan: Install-state portability (shared GRIM_HOME / devcontainers)

> Preview only. No workers launched, no code written, no plan finalized
> until approved at the gate. Cost-transparency per `/swarm-plan` step 4.

## Target

Implement the redesign in
[`.agents/adr/adr_install_state_portability.md`](../adr/adr_install_state_portability.md):
relocate + anchor-relativize install state so it survives a `/workspace`
bind mount and a shared `GRIM_HOME`, and drop the denormalized top-level
record fields (the "`.copilot` twice" smell).

Free-text target (from the `/architect` session). **GitHub:** no related
open issue/PR (only dependabot #8/#10). Recommend filing a tracking issue
on approval.

## Classification

- **Tier:** `high` + overlays — **low-confidence** (signals straddle high↔max)
- **Scope:** Medium (≈8–10 source files + tests + docs)
- **Reversibility:** One-Way Door **Medium–High** (on-disk format + user-data migration)
- **Overlays:**
  - `--architect=opus` — trigger: "content-addressed storage layout change" + cross-subsystem
  - `--research=3` — trigger: security-sensitive area (path-traversal boundary)
  - `--codex` (plan-artifact) — trigger: One-Way Door + security-sensitive

**Why not `max`:** no new crate, no public CLI/API surface change, no
protocol change — it is an internal refactor of one subsystem's on-disk
format. `max` would mandate `pr_faq` + `prd` (customer narrative), which is
ceremony for internal infra. The ADR (max's one mandatory design artifact)
already exists. So: `high` base with `max`-grade design rigor (opus
architect, 3-axis research, Codex gate) minus the customer-narrative docs.

**Why not `low`:** on-disk migration of real user state + a security
boundary rule out a Two-Way-Door treatment.

## Workers I would launch (per phase)

| Phase | Worker(s) | Model | Count | Purpose |
|---|---|---|---|---|
| 1 Discover | (reuse ADR discovery) + `worker-explorer` | haiku | 1 | Confirm migration touch-points: `scope_resolution`, `uninstall`, `status`, OpenCode `sync_config`, `.gitignore` packaging |
| 2 Research | `worker-researcher` ×3 | sonnet | 3 | Axis A: path-anchor/relativization patterns + traversal-safe rejoin (Rust). Axis B: lockfile-vs-state split precedent (cargo/npm/poetry) + devcontainer bind-mount conventions. Axis C: on-disk schema migration/versioning (serde, forward-compat) |
| 4 Design | `worker-architect` | **opus** | 1 | Turn ADR into testable component contracts: `PathAnchor`/`AnchoredPath` API, resolve+validate spec, migration state machine, error taxonomy, edge cases |
| 6 Review | `worker-reviewer` (spec-compliance) | sonnet | 1 | Contracts testable? match UX/migration scenarios? |
| 6 Review | `worker-architect` (trade-off honesty) | opus | 1 | One-Way-Door rigor; subsystem-boundary check |
| 6 Review | `worker-researcher` (SOTA gap) | sonnet | 1 | Missed pitfalls in migration/traversal handling |
| 6 Codex | `codex-adversary` (plan-artifact) | — | 1 | Cross-model final gate on plan file |

Max concurrency 8; Round-1 review panel runs concurrent. Workers never
commit (report `git status` only).

## Artifacts I would produce

- `.agents/plans/plan_install_state_portability.md` (executable phases + `## Status` block)
- `.claude/state/current_plan.md` (pointer)
- `.agents/research/research_state_portability.md` (3-axis findings, persisted)
- ADR already exists — Design phase **refines** it (contracts/migration), not a new ADR
- (optional, on approval) GitHub tracking issue

## Estimated cost

- Parallel workers: peak 3 (research) then 3 (review panel) concurrent
- Heaviest calls: 1× opus architect (design) + 1× opus reviewer + 3× sonnet research
- Codex plan review: **on** (1 one-shot pass; skipped gracefully if unavailable)
- Order of magnitude: ~9 worker invocations across phases

## Not doing

- No implementation code (that is `/swarm-execute`)
- No commits, no push, no PR creation
- No `pr_faq`/`prd` (internal infra; see "Why not max")
- No change to the **committed lock** format (ADR Option 1 keeps lock untouched)
