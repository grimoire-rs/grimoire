# Tier: max — /swarm-review

Full adversarial kitchen sink for big diffs (>15 files, cross-subsystem, new crate, breaking API, protocol change, security-sensitive paths, or `breaking-change` / `epic` label). Add `worker-architect` (SOLID, boundary, dependency direction) and `worker-researcher` (SOTA gap check) to Stage 2 panel, apply Five Whys RCA to every finding above Suggest, run Codex cross-model pass as mandatory final gate before verdict.

Load file via `Read` from `SKILL.md` after config announced.

**Auto meta-plan preview**: when tier resolves to `max`, SKILL.md step 5 auto-fires meta-plan gate (opt out with `--no-dry-run`). Cost transparency — max-tier runs expensive, catches misclassifications before tokens burn.

## Phase 1: Discover

Read diff against resolved baseline. Parse file list, map paths to subsystems — **all**, including adjacent subsystems possibly affected by cross-cutting changes. Read:

- `subsystem-*.md` rules for every touched subsystem
- Relevant ADR (`.claude/artifacts/adr_*.md`) if one covers diff topic
- `.claude/rules/product-context.md` — diff may imply positioning shifts review must catch
- Language quality rules matching diff languages

**Gate**: Diff fetched, full context loaded, product-context read.

## Phase 2: Stage 1 — Correctness (parallel, 3 workers)

> **Reviewer model**: every `worker-reviewer` launch in this tier uses resolved `--reviewer` overlay value (tier=max default `sonnet`; escalated to `opus` when `--breadth=adversarial` fires). See `overlays.md` reviewer axis.

Same as tier-high — launch **in single message with multiple Agent tool calls** so they run concurrently:

- **1** `worker-reviewer` (focus: `spec-compliance`, phase: `post-implementation`) — reviews **Implement**-phase output against Grimoire anchors
- **1** `worker-reviewer` (focus: `quality`, lens: test-coverage) — checks **Specify**-phase tests cover edge cases, boundary conditions, concurrent access, failure modes
- **1** `worker-reviewer` (focus: `compatibility`) — breaking-change gate: diff must not break released surfaces (CLI, JSON output, schemas, layouts, exit codes). Any breaking change = Block-tier — prohibited during the 1.0.0 stabilization freeze. Contract: `docs/src/stability.md`. Max-tier diffs carry `breaking-change` labels or protocol signals most often — this reviewer never skipped.

At max tier, **Stub** / **Specify** / **Implement** lifecycle traceability extra important — reviewer notes any implementation behavior with no corresponding test or design-record anchor.

**Gate**: All three reviewers complete.

## Phase 3: Stage 2 — Adversarial breadth (parallel, up to 6 workers)

Launch **in single message with multiple Agent tool calls** so they run concurrently:

- `worker-reviewer` (focus: `quality`) — include CLI-UX lens when diff touches command surface
- `worker-reviewer` (focus: `security`) — always at max (assume security-sensitive until proven otherwise)
- `worker-reviewer` (focus: `performance`) — always at max
- `worker-doc-reviewer` — always at max (doc drift at scale is default failure mode); model per resolved `--doc-reviewer` overlay (`sonnet` default; `haiku` when narrow-scope doc trigger fires — see `overlays.md` doc-reviewer axis)
- `worker-architect` — SOLID, subsystem boundary respect, dependency direction, trade-off honesty; check diff against any ADR covering area
- `worker-researcher` — SOTA gap: how do leading tools (Cargo, npm, pip/uv, Go modules, Helm) solve same problem? Algorithm choice current? Known pitfalls unaddressed?

Stage 1 (3) and Stage 2 (6) run as separately gated batches — each batch under the 8 concurrent worker ceiling. If diff clearly doesn't need `worker-researcher` (e.g., pure refactor, no algorithmic change), skip it, stay at 5 in Stage 2.

Each reviewer classifies findings as actionable / deferred / suggest.

**Gate**: All perspectives complete.

## Phase 4: Root-cause analysis (rca=on, all findings above Suggest)

Apply Five Whys to every Block, High, Warn finding. Max-tier coverage deliberately wider than high-tier — big cross-subsystem diffs often share systemic causes. Clustering findings by root reveals patterns (e.g., "three findings all trace back to missing async cancellation guard in worker pool").

```
**Issue**: [problem]
**Why 1** … **Why 5**: [causal chain]
**Systemic Fix**: [what prevents recurrence]
**Related findings**: [list of other findings sharing this root]
```

**Gate**: RCA complete for all findings above Suggest. Clusters noted.

## Phase 5: Cross-model — Codex (mandatory)

Invoke `codex-adversary` with scope `code-diff --base <base> --model sol` (`gpt-5.6-sol`; `--codex-model` overrides) against branch diff. One-shot, no looping.

Triage per `overlays.md`:

- **Actionable** → reported in Cross-Model section of output. Review read-only — no builder fix pass.
- **Deferred** → added to Deferred Findings with reason
- **Stated-convention** → dropped, count mentioned
- **Trivia** → dropped, count mentioned

Unavailable path: at max-tier this is **gate, not blocker** — surface skip prominently in verdict so reader knows one review layer missed. Log `Cross-model gate skipped: <reason>`, include in Summary line.

**Gate**: Codex triage complete (or skip surfaced).

## Phase 6: Verdict & Output

Produce review report using shared skeleton from `SKILL.md`:

```markdown
## Code Review: [target]
### Summary
- Verdict: [Approve / Needs Work / Request Changes]
- Tier: max
- Baseline: <base>
- Diff: N files, +L / -L lines, S subsystems
- Cross-model: [ran | skipped: <reason>]
### Stage 1 — Correctness
#### Spec-compliance (post-Implement traceability)
#### Test Coverage (Specify-phase adequacy)
#### Compatibility (breaking-change gate)
### Stage 2 — Adversarial panel
#### Quality
#### Security
#### Performance
#### Documentation
#### Architecture
#### SOTA / Technical Soundness
### Cross-Model Adversarial (Codex)
### Root-Cause Analysis
[Clusters with systemic fixes]
### Deferred Findings
```

**Verdict rules**:
- **Request Changes** if any Block-tier finding unresolved; any security vulnerability; any architect-flagged boundary violation; any breaking change (prohibited during the 1.0.0 stabilization freeze); tests absent for new behavior; systemic cause affecting ≥3 findings
- **Needs Work** if Warn-tier findings exist or Cross-model pass surfaced actionable findings not yet addressed
- **Approve** otherwise

At max-tier, explicitly surface in Summary:
- Whether Codex gate ran or skipped (with reason)
- Architect-flagged boundary or ADR-compliance concerns
- SOTA gaps researcher flagged

**Gate**: Report printed. No commits.

## Handoff

Standard handoff from `SKILL.md`. Classification line:

```
- Scope: Large (One-Way Door High)
- Tier: max
- Baseline: <base>
- Overlays: breadth=adversarial, rca=on, codex=on
```

If actionable findings exist:

    /swarm-execute max .claude/state/plans/plan_[feature].md
    /swarm-execute max "apply max-tier review findings"   # no plan yet

If SOTA gaps or architectural concerns need own ADR, escalate:

    /architect "propose ADR for [concern]"