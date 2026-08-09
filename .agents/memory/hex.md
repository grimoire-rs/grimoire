# hex — swarm memory

Maintained by the hex skills. Small by contract: pointers and
preferences, not copies. Team-shared — commit it.

## Pointers

- Verification: `AGENTS.md` › "Build & Development Commands" — run
  `task verify` (full gate) before any merge; `task rust:verify`,
  `task shell:verify`, `task claude:verify` are the per-subsystem dev-loop
  gates; `task catalog:verify` gates first-party catalog drift.
- Plan / ADR / spec conventions: `AGENTS.md` › "Workflow" › Planning flow.
  ADRs `.agents/adr/`, design specs `.agents/specs/`, plans (incl.
  `bugfix_plan_*`, `meta-plan_*`) `.agents/plans/`, research
  `.agents/research/`; one-off records at `.agents/` root. Templates:
  `.claude/templates/artifacts/` — the project's own, not hex's fallbacks.
- Spec home: `.agents/specs/` — the fold-back target; default ID-marker
  heading shape (no project-specific marker declared).
- Plan Status block: every `.agents/plans/plan_*.md` carries one; schema
  and per-skill mutation table in `.claude/rules/meta-ai-config.md` ›
  "Plan Status Protocol". Fast-path pointer `.claude/state/current_plan.md`
  is gitignored and per-worktree.
- Product knowledge: `.claude/rules/product-context.md` (canonical
  identity, related repos, comparable tools, research keywords; indexed
  from `AGENTS.md` › "Project Identity").
- Rule catalog: `.claude/rules.md` — "By concern" table routes to the rule
  for any given task before a file is open. Read it before planning.
- Key rules: `.claude/rules/arch-principles.md` (boundaries, invariants);
  four `src/**`-scoped subsystem rules (`subsystem-cli.md`,
  `subsystem-cli-api.md`, `subsystem-cli-commands.md`,
  `subsystem-file-structure.md`) co-fire by design;
  `.claude/rules/quality-security.md` for security-sensitive surfaces;
  `.claude/rules/vendor-capability-watchlist.md` before patching a vendor
  renderer decline.
- Security-sensitive paths: `src/oci/**` (registry transport, auth),
  `src/command/login.rs` / `logout.rs` (docker-config credential
  read/write), `src/command/publish*` / `release*` and `catalog/**` (push
  bytes to a public registry). See the `perspectives.always` rules below.
- Constitution: `AGENTS.md` › "Core Principles" — nine binding principles.
  Principle 9 (Preserve Compatibility) is a hard gate on the road to
  1.0.0: breaking changes are prohibited, evolution is additive-only.
  Contract detail: `docs/src/stability.md`,
  `.agents/adr/adr_render_layout_stability.md`.
- Worktrees: agent worktrees at the hex default `.agents/worktrees/`
  (gitignored); human feature worktrees are siblings `../grimoire-wt-<topic>`
  (`AGENTS.md` › "Workflow"). Whoever creates one removes it.

## Preferences

```yaml
# hex config, vocabulary v2. Unknown keys warn once and are ignored.
models:
  fast-balanced: sonnet
  deep-reasoning: opus
  overrides:
    # Mirrors the owner's global model-routing policy: review and
    # non-mechanical implementation run deep at every tier.
    reviewer:security: deep-reasoning
    reviewer:quality: deep-reasoning
    reviewer:spec: deep-reasoning
    builder:implement: deep-reasoning
adversary: codex:rescue

perspectives:
  always:
    # Registry transport + credential read/write.
    - role: reviewer:security
      when: "src/oci/**"
    - role: reviewer:security
      when: "src/command/{login,logout}.rs"
    # Anything that pushes bytes to a public registry.
    - role: reviewer:security
      when: "src/command/{publish,release}*"
    - role: reviewer:security
      when: "catalog/**"
    # AGENTS.md: CLI changes require a catalog + docs drift review.
    - role: doc-reviewer
      when: "src/command/**"

research-axes:
  - OCI spec evolution (artifactType, subject/referrers, empty-config compat)
  - registry ecosystems (GHCR, Docker Hub, GitLab, Harbor) and their gaps
  - vendor AI-config layouts and capability differences
  - skill / rule / agent authoring conventions across clients
  - package-manager UX and lockfile design
  - discovery and indexing without a hosted service
```

- Security review here means the registry transport, credential handling,
  and publish paths named under Pointers — not the render/format code that
  merely passes bytes through.
- Stability is a review perspective in its own right: any diff touching a
  schema, an install layout, or a renderer is checked against Principle 9
  before anything else, regardless of tier.

## Memory

_(empty at bootstrap — the orchestrators maintain this section.)_
