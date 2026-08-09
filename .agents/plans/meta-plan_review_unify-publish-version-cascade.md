# Meta-Plan: /swarm-review — feat/unify-publish-version-cascade

## Classification

- **Tier:** max (auto)
- **Rationale:** breaking change (`feat(publish)!` — `--tag` removed, channel
  re-publish now needs `--force`); cross-subsystem (src CLI + src OCI + test +
  docs + catalog); 1029 lines (+739/−290) > 500 high ceiling; core version/
  cascade logic rewritten (`publish.rs` +450, `oci/release.rs` +213).
- **Confidence:** high (size + breaking marker both point max; no competing
  signal). Max auto-fires this gate regardless.

## Diff metrics snapshot

- 13 files, +739 / −290
- Subsystems: src (CLI + OCI), test (acceptance), docs, catalog, .claude
- Structural markers: breaking API (One-Way Door High), registry I/O path
  (`src/oci/release.rs`)

## Baseline

- `main` (default — no `--base`, no PR target)
- Target: `HEAD` (branch `feat/unify-publish-version-cascade`, 1 squashed commit)

## Overlays (max defaults)

| Axis | Value | Source |
|---|---|---|
| breadth | adversarial | tier default |
| reviewer | opus | adversarial breadth fires opus |
| doc-reviewer | sonnet | commands.md is primary reference (not narrow trigger) |
| rca | on (all >Suggest) | tier default |
| codex | on (mandatory) | tier default |

## Workers (8 total, ≤8 ceiling)

**Stage 1 — Correctness (2):**
- worker-reviewer (spec-compliance, post-implementation) — impl vs ADR contract
- worker-reviewer (quality, lens: test-coverage) — cascade/channel/force edge cases

**Stage 2 — Adversarial panel (6):**
- worker-reviewer (quality + CLI-UX lens) — flag surface, error messages
- worker-reviewer (security) — tag overwrite / force semantics, injection at boundary
- worker-reviewer (performance) — OCI push paths, N+1 tag pushes in cascade
- worker-doc-reviewer — docs/catalog drift vs new `--version`/`--cascade` surface
- worker-architect — SOLID, publish/release boundary, ADR compliance
- worker-researcher — SOTA: how npm/cargo/helm/OCI handle semver cascade vs channel tags

**Phase 5 — Codex cross-model** (mandatory final gate, code-diff scope).

## Estimated cost

~8 parallel workers (opus reviewers) + RCA pass + Codex. Heavy. Max tier.

## Not Doing

- No auto-fixes, no commits (review = read-only)
- No pushing to remote
- Findings reported + classified actionable/deferred; handoff to /swarm-execute if fixes wanted
