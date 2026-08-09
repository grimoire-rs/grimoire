# Meta-Plan: /swarm-review feat-codex-vendor (round 2, post-rebase)

## Classification

- **Tier:** max (auto, confident)
- **Rationale:** 40 files / +1498 −118 lines / ≥3 subsystems (src
  install+CLI, test acceptance, docs, catalog, .claude config). Structural
  marker: new client-vendor contract — `CodexVendor`, `Vendor` trait
  changes, first TOML-emitting vendor, cross-vendor `~/.agents/skills`
  standard. One-way-door-ish (published vendor target hard to retract).
- **Diff metrics snapshot:** files=40, +1498/−118, subsystems≥3.
- **Round context:** branch freshly rebased on main; prior max-tier review
  ran pre-rebase (28 files). This round re-validates approach post-rebase.

## User focus (this round)

1. Is Codex-as-fourth-client-vendor approach still right after rebase?
2. Do ALL artifact types (skills, rules, agents, MCP, bundles) have good
   Codex support — or documented, justified gaps?
3. Online research: current Codex CLI conventions (AGENTS.md, ~/.codex,
   skills/prompts support, MCP config format) — is the ADR's model of
   Codex still accurate?

## Baseline

- **main** (default — no `--base`; branch is 1 feature commit `3c6c0c0`).

## Overlays (max defaults)

- **breadth = adversarial** — Stage 2 adds architect (SOLID/boundary vs
  `adr_codex_vendor.md`) + researcher (SOTA gap + Codex-conventions web
  research) + reviewer CLI-UX lens, atop quality/security/perf/docs.
- **reviewer = opus** (adversarial breadth at max, per overlays.md).
  Override available: `--reviewer=sonnet`.
- **doc-reviewer = sonnet** (7 doc files incl. user guide → not the
  narrow ≤2-file haiku trigger).
- **rca = on** (all findings above Suggest).
- **codex = on** (mandatory cross-model gate at max, model `sol`).

## Workers per perspective

- Stage 1 (correctness, 2 parallel): spec-compliance (post-implementation,
  vs ADR) + test-coverage.
- Stage 2 (adversarial, 6 parallel): quality+CLI-UX, security,
  performance, documentation, architecture (ADR compliance + artifact-type
  coverage matrix), SOTA/researcher (web: Codex CLI conventions 2026).
- Cross-model: codex-adversary (code-diff, --base main, model sol),
  one-shot.

## Estimated cost

~9 agents (opus reviewers, sonnet researcher/doc) + 1 Codex pass.
Read-only.

## Not Doing

- No auto-fixes. No commits. No push. Review read-only; findings
  classified actionable / deferred only.
