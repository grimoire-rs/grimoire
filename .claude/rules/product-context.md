---
paths:
  - docs/**
  - .agents/plans/**
  - .agents/adr/**
  - .agents/specs/**
  - .agents/research/**
  - .claude/skills/builder/**
  - .claude/skills/code-check/**
  - .claude/skills/qa-engineer/**
  - .claude/skills/security-auditor/**
  - .claude/skills/docs/**
---

# Grimoire Product Context

> A package manager for AI-agent config — skills, rules, agents, and MCP
> servers, installed into every coding agent you use.
>
> Sentence two, verbatim wherever the headline appears: *Storage is any OCI
> registry — GHCR, Docker Hub, or your own. There is no Grimoire service to
> sign up for.*

> **Status: stabilizing — preparing 1.0.0.** Released surfaces are frozen;
> breaking changes are prohibited (stability contract:
> `adr_render_layout_stability.md`, `docs/src/stability.md`). Statements
> below are maintained positioning — flag drift via the Update Protocol
> at the bottom of this file.

Grimoire (binary: `grim`) is a CLI for installing, maintaining, and
publishing AI-agent configuration — skills, rules, prompts, and related
artifacts — distributed through standard OCI registries. The relationship
to OCI is analogous to how a binary package manager reuses container
registries: any Docker/OCI registry becomes a distribution channel for
reusable AI config, with no bespoke server to operate.

This rule is the canonical product identity. Read it when reasoning about
project direction, trade-offs, ADR motivation, research framing, doc
narratives, or positioning.

## The Problem

Reusable AI-agent configuration (skills, rules, hooks, prompt templates)
today tends to be copy-pasted between repositories with no versioning,
provenance, or update path. There is no common, infrastructure-light way
to publish a skill once and install or upgrade it across many projects.

## Why OCI

- **Zero infrastructure cost** — reuse a registry you already run
- **Auth / RBAC / TLS for free** — inherit the registry security model
- **Standards-based** — stable, widely adopted, vendor-neutral
- **Ecosystem tooling** — scanning, replication, GC already exist

## Discovery: The Index

Registries answer "give me this package", not "what packages exist?" —
`_catalog` is gated or absent on GHCR, Docker Hub, and GitLab SaaS. A
**package index** fills that gap: a repository of pointers grim browses.

It is deliberately not a service. `npx @grimoire-rs/indexer init`
scaffolds an index repository — pointer tree, catalog site, contribution
gate — that GitHub or GitLab Pages serves, so a team stands up its own
discovery surface with no server and no account. The public index at
`index.grimoire.rs` is the default, not the system. This is the current
lead adoption story on the landing page and in
`docs/src/hosting-an-index.md`.

## Target Users

- **Primary**: Engineers maintaining AI-agent configuration shared across
  multiple repositories or teams
- **Secondary**: Platform teams curating an internal catalog of approved
  skills and rules
- **Non-target**: One-off, single-repo config that never needs to be shared

## Product Principles

1. **Backend-friendly** — JSON output, composable commands, clean exit codes
2. **Offline-first** — a local index/cache should make repeat operations
   work without network access
3. **Content-addressed** — immutable, deduplicated artifact storage
4. **Zero infrastructure cost** — bring your own OCI registry
5. **Private-first** — registry auth is first-class; internal catalogs are
   as easy to use as public ones

## CLI at a Glance

```bash
grim add ghcr.io/acme/code-review:1  # Declare + lock + install an artifact
grim install                         # Materialize the locked set into clients
grim status                          # Per-artifact state (+ outputs in JSON)
grim update                          # Re-resolve floating tags, roll forward
grim release ./my-skill some/skill:1 # Push a single artifact to a registry
grim publish                         # Batch-release packages from publish.toml
grim uninstall skill code-review     # Full inverse of install
```

Global flags: `--offline`, `--global`, `--config <path>`,
`--registry <ref>`, `--format json`. Full surface:
`subsystem-cli-commands.md` and `docs/src/commands.md` (23 subcommands).

## Technical Overview

- **Language**: Rust 2024
- **Layout**: single binary crate — source lives under `src/`; the binary
  is `grim`, the crate/package is `grimoire`. No lib/CLI split, no workspace.
- **Default registry**: configurable via `GRIM_DEFAULT_REGISTRY`
- **Testing**: pytest acceptance tests under `test/` against a real OCI
  registry

## Related Repositories

| Repo | Relationship |
|---|---|
| `grimoire-vscode` | VS Code extension — browses the catalog and drives `grim` through its JSON interface |
| `grimoire-index` | The public package index served at `index.grimoire.rs` — a phone book of pointers, not a catalog |
| `grimoire-components` | Reusable CI/CD components for grim-based pipelines |
| `arcana` | First-party skill/agent bundles published *with* grim (e.g. the `hex` swarm bundle) |
| `external/rust-oci-client` | OCI transport, `ocx-sh` fork, submodule pinned to `ocx/integration` |
| `external/docker_credential` | Credential-helper reads, `ocx-sh` fork, submodule pinned to `feat/store-erase-list` |
| `ghcr.io/grimoire-rs/*` | Published first-party catalog packages (source in `catalog/`) |
| `ocx-sh/index` | Sibling prior art — sparse static HTTP index for the OCX package manager; index-tooling architecture reference |

## Comparable Tools

Verified landscape, with signal counts and gaps, lives in
`.agents/research/research_promotion_positioning.md` › "Competitive
landscape" (re-verify after 2027-01-26). Nearest comparables: Vercel
`skills.sh`, Tessl, ClawHub, `skillctl`, `jeffreytse/grimoire`. Structural
prior art rather than competitors: ORAS (OCI artifact transport), Helm's
OCI chart distribution, npm/cargo lockfile UX, Claude Code plugin
marketplaces. The differentiator ranking lives in that same artifact —
do not restate it here, it moves fast.

## Research Keywords

For researchers scoping a new axis: `OCI artifactType` + `subject`
referrers · `agent skills specification` / `agentskills.io` · `OCI Agent
Skills Artifacts` (Vitale draft spec) · `AGENTS.md` / `CLAUDE.md`
cross-vendor rule formats · `claude code plugin marketplace.json` ·
registry `_catalog` gating on GHCR / Docker Hub / GitLab · lockfile and
digest pinning · `SKILL.md` GitHub code search. Each
`.agents/research/research_*.md` ends with its own durable search-term
list — check there before inventing queries.

## Update Protocol

This file is the single source of truth for Grimoire product identity.
Stale positioning degrades every downstream decision (ADRs, research
framing, doc narratives). Keep it current.

**When to update** — any of these trigger an edit in the same commit:

1. The product vision is fleshed out or revised
2. Target user shift (primary / secondary / non-target list change)
3. A product principle is added, dropped, or reworded
4. A scope decision reframes positioning
5. A CLI-level UX change visible to positioning

**Who must check** — any agent working at product level re-reads this file
when work could shift positioning: a researcher after evaluating a
library/tool; an architect after an ADR or design spec; a doc writer after
user-guide edits; a builder or reviewer if implementation exposes a
capability gap or breaks a stated principle.

**Validation** — `/meta-maintain-config refresh` spot-checks this file
against current CLI help, source code, and recent ADRs.
