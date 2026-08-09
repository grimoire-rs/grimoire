# Meta-Plan: /swarm-review feat/vendor-capability-unlock

## Classification

- **Tier:** max (auto — confident)
- **Rationale:** 26 files > 15-file max threshold; ≥2 subsystems
  (src/install + src/oci + src/command, test/, docs/, .claude/, catalog/);
  core data-flow modules touched (installer.rs reaper, oci/mcp.rs schema).
- **Diff metrics:** 26 files, +1751 / −155 lines, 5 areas.
- **Structural markers:** core `src/**` data-flow (installer reap path,
  MCP descriptor schema + validation). No new crate, no dep changes,
  no public-API removal (additive-only by design).

## Baseline

- **Base:** `main` (default — no `--base`, no PR)
- **Target:** HEAD (branch `feat/vendor-capability-unlock`, 18 commits)

## Overlays (all tier defaults, no user flags)

| Axis | Value | Source |
|---|---|---|
| breadth | adversarial | max default |
| reviewer | opus | max + adversarial breadth escalation |
| doc-reviewer | sonnet | 8 doc/config .md files touched incl. user guide — narrow-scope haiku trigger does not fire |
| rca | on (all > Suggest) | max default |
| codex | on (mandatory), model=sol | max default |

## Workers

- **Stage 1 (2 parallel, opus):** spec-compliance (post-implementation,
  vs plan zippy-crunching-cray + adr_render_layout_stability),
  quality/test-coverage (reaper guards, upgrade fixture, MCP compat locks).
- **Stage 2 (6 parallel):** quality+CLI-UX (opus), security (opus — oauth
  block secret-handling, header env-ref mapping), performance (opus),
  doc-reviewer (sonnet — vendor-metadata/mcp-servers/stability + catalog
  drift), architect (opus — compat doctrine adherence, reaper design vs
  ADR, additive-schema discipline), researcher (sonnet — vendor upstream
  claims spot-check: xhigh, Codex header surfaces, Claude ws/oauth).
- **Phase 4:** Five Whys RCA on all findings above Suggest, clustered.
- **Phase 5:** Codex cross-model gate, scope code-diff --base main, model sol.

## Estimated cost

8 workers (2+6) + 1 Codex pass; opus-heavy reviewer set on a
~1.9k-line diff. Heaviest review profile available — matched to a
26-file cross-subsystem branch introducing a destructive code path
(reaper) and schema surface growth.

## Not Doing

- No auto-fixes, no commits, no push — review is read-only.
- No re-verification runs beyond reviewers' own spot checks
  (full `task verify` already green on branch).
