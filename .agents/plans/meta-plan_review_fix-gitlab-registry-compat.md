# Meta-Plan: /swarm-review — fix/gitlab-registry-compat

## Classification

- **Tier:** max (confident)
- **Rationale:** 24 files / +1172 −91 (1263 lines) across 5+ subsystems
  (oci, command, install, catalog, tui) + tests + docs + .claude. Structural
  markers: **registry I/O path** (`registry_client.rs`) + **OCI wire/protocol
  change** (config descriptor media type, dropped `artifactType`) = One-Way
  Door High signal. `>15 files` OR protocol change → max by metrics; both fire.
- **Diff metrics snapshot:** 24 files, +1172 / −91, ≥5 subsystems.

## Baseline

- **Base:** main (default — no `--base` flag, target = current branch HEAD)
- **Target:** HEAD (branch: fix/gitlab-registry-compat, 9 commits ahead)

## Overlays (max defaults)

| Axis | Value | Source |
|---|---|---|
| breadth | adversarial | tier default |
| reviewer | opus | tier=max + adversarial breadth (overlays.md reviewer axis) |
| doc-reviewer | sonnet | doc diff touches primary user guide (configuration.md, publishing.md) — not narrow scope |
| rca | on (all findings > Suggest) | tier default |
| codex | on (mandatory final gate) | tier default |

## Workers per perspective

**Stage 1 — Correctness (2 parallel):**
- worker-reviewer (spec-compliance, post-implementation) — diff vs plan/ADR anchors
- worker-reviewer (quality, lens: test-coverage) — regression-test adequacy

**Stage 2 — Adversarial panel (6 parallel):**
- worker-reviewer (quality, +CLI-UX lens — publish/search command surface touched)
- worker-reviewer (security — wire format, repository-path validation, input handling)
- worker-reviewer (performance)
- worker-doc-reviewer (sonnet — docs/ADR/catalog drift vs code)
- worker-architect (SOLID, OCI subsystem boundaries, ADR-compliance vs adr_oci_empty_config_compat.md)
- worker-researcher (SOTA: how ORAS / Helm / Cargo handle OCI media-type + registry-namespace compat)

8 workers total — at concurrency ceiling.

## Then

- RCA: Five Whys on every Block/High/Warn finding; cluster by root cause.
- Codex: `codex-adversary code-diff --base main` — mandatory final gate; skip surfaced if unavailable.
- Verdict + report. **No commits, no auto-fix** (review read-only).

## Not Doing

- No auto-fixes, no commits, no push.
- No closing #11.
- Findings reported only — handoff to /swarm-execute or /finalize is the user's call.

## Estimated cost

8 subagents (1 opus architect, opus reviewers, sonnet doc/researcher) + 1 Codex pass. Large token spend — max tier. Opt out by re-running with explicit lower tier (`/swarm-review high`).
