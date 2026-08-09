# Plan: First-Party Catalog — Grim Skills, Authoring Knowledge & Publish Tooling

## Status

- **Plan:** plan_catalog_packages
- **Active phase:** 4 — Repo integration (complete)
- **Step:** awaiting /swarm-review
- **Last update:** 2026-06-12 (after 18c046c: chore(claude): register catalog subsystem in rules and drift reminders)

## Context

grim v0.4.0 released; nothing teaches agents (a) how to use grim well or (b) how to author good AI config — generally and grim-specifically. Dogfood: ship grim's own knowledge as grim-publishable packages in top-level `catalog/`, validated by `grim build` in CI, published to `grim.ocx.sh` via manual workflow. Tree-structure pattern throughout: root SKILL.md = short index/bootstrap; deep knowledge in `references/` files loaded on demand.

Full approved plan: `~/.claude/plans/typed-wondering-leaf.md` (mirrored below in condensed form).

## Packages

| Package | Name | Purpose |
|---|---|---|
| Skill | `grim-usage` | Drive grim CLI: consume + publish lifecycles, registries, troubleshooting |
| Skill | `ai-config-authoring` | Vendor-neutral craft: per-type design deep-dives (skill/rule/agent), type decision table, progressive disclosure, descriptions |
| Skill | `grim-authoring` | Grim-specific: per-kind frontmatter schemas, validation pitfalls, vendor-metadata projection |
| Bundle | `grim-essentials` | Ties all 3 together |

All skills: Apache-2.0, `metadata.summary`/`keywords`, `repository: https://github.com/michael-herwig/grimoire`, `compatibility: grim>=0.4` (grim-* only), no vendor keys in v1. Refs: `grim.ocx.sh/skills/<name>`, `grim.ocx.sh/bundles/<name>`; semver from 1.0.0, cascade tags; bundle members on `:1` floating, no `--pin`.

## Phases

### Phase 1 — Research (6 parallel researchers; persist before authoring)

Deep analysis: every type (skill, rule/instruction, agent) × every vendor (Anthropic/Claude, OpenCode, Copilot), online. Artifacts:

| # | Focus | Artifact |
|---|---|---|
| R1 | Anthropic/Claude skills best practices | `.agents/research/research_skills_anthropic.md` |
| R2 | Anthropic/Claude rules & agents | `.agents/research/research_rules_agents_anthropic.md` |
| R3 | OpenCode + Copilot, all types; agentskills.io spec | `.agents/research/research_opencode_copilot_types.md` |
| R4 | Community norms, CLI-teaching skills | `.agents/research/research_community_skill_packs.md` |
| R5 | Type decision table (skill vs rule vs agent vs hook) | `.agents/research/research_artifact_type_decision.md` |
| R6 | In-repo distillation map (no web) | `.agents/research/research_inrepo_authoring_distillation.md` |

Each returns: canonical further-reading links + durable search terms (feed `references/updating.md` files).

### Phase 2 — Content

Layout: `catalog/{README.md,publish.toml,scripts/publish.py,taskfile.yml}`, packages under `catalog/skills/<name>/` (SKILL.md + `references/*.md` per agentskills.io spec; `scripts/`/`assets/` when needed), bundle at `catalog/bundles/grim-essentials.toml`. `catalog/rules/`, `catalog/agents/` when first such package lands.

- Authoring order: ai-config-authoring → grim-usage → grim-authoring → bundle + README (dogfood check: each package follows its own rules).
- ai-config-authoring references: choosing-types, skill-design, rule-design, agent-design (in-depth, research-backed, vendor notes + Further Reading links), descriptions, guardrails, checklist, updating.
- grim-usage references: consume, publish, registries, troubleshooting, updating.
- grim-authoring references: skill/rule/agent/bundle-spec, vendor-metadata, release-checklist, updating.
- Every skill: `references/updating.md` = re-research procedure + search terms + canonical links + drift-tier reminder.
- Drift tiers (in README): 1 inline invariants; 2 summarize + "confirm with `grim <cmd> --help`"; 3 link-only (vendor registries, limits). Verify-before-acting protocol atop grim-* SKILL.md; "Verified against grim 0.4.x" footer.

### Phase 3 — Tooling

- `catalog/taskfile.yml`: `.ensure-grim` (deps `:rust:build`, SKIP_BUILD status), `verify` (grim build over `skills/*/` + `bundles/*.toml`, GRIM_COMMAND override), `release` (wraps `scripts/publish.py`). Root taskfile: include + `catalog:verify` in `.verify:build-test` after `rust:build`.
- `catalog/scripts/publish.py` (PEP 723, stdlib): parse publish.toml, skills before bundles, `grim release <path> <ref>`, `--dry-run`/`--force` passthrough. Versions per package in publish.toml, no git tags (cliff tag_pattern unanchored).
- CI validate: one step in verify-basic.yml smoke job: `task catalog:verify`.
- `.github/workflows/publish-catalog.yml`: workflow_dispatch only (dry_run/force/grim_version), `environment: grim.ocx.sh` (REGISTRY_USER/REGISTRY_TOKEN via env intermediary), DOCKER_CONFIG=$RUNNER_TEMP/docker, install released grim via gh release download, `grim login --password-stdin --allow-insecure-store`, always dry-run smoke, publish gated, `grim logout` if always(). Pattern: publish-ocx.yml + docs/src/authentication.md CI recipe.

### Phase 4 — Repo integration

Update subsystem-taskfiles.md, subsystem-ci.md, `.claude/rules.md` (same commit; structural tests enforce). catalog/README.md: local loop, version-bump procedure, drift tiers, update-protocol note.

## Verification

1. `task catalog:verify` (grim build = real schema gate); 2. `task verify` + `task claude:tests`; 3. content audit (line targets, descriptions per meta-ai-config, links resolve, updating.md present, design files cite research artifacts); 4. `task catalog:release -- --dry-run`; 5. scratch-project install smoke across claude/opencode/copilot; 6. CI publish dispatch with dry_run from branch; 7. /swarm-review then /finalize. Publish = human-triggered.

## Risks

GNU/BSD find portability; released-vs-HEAD grim skew (dry-run surfaces); content drift (tiers + CI grim build + updating.md).
