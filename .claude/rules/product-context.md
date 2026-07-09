---
paths:
  - docs/**
  - .claude/artifacts/**
  - .claude/agents/worker-researcher.md
  - .claude/agents/worker-architect.md
  - .claude/agents/worker-doc-writer.md
  - .claude/skills/architect/**
  - .claude/skills/builder/**
  - .claude/skills/code-check/**
  - .claude/skills/qa-engineer/**
  - .claude/skills/security-auditor/**
  - .claude/skills/swarm-execute/**
  - .claude/skills/swarm-plan/**
  - .claude/skills/swarm-review/**
  - .claude/skills/docs/**
---

# Grimoire Product Context

> An OCI-backed package manager for AI skills and rules.

> **Status: active.** De-provisionalized on the road to 1.0.0 (see
> `adr_render_layout_stability.md` and `docs/src/stability.md` for the
> stability contract). Statements below are maintained positioning —
> flag drift via the Update Protocol at the bottom of this file.

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
`subsystem-cli-commands.md` and `docs/src/commands.md` (18 subcommands).

## Technical Overview

- **Language**: Rust 2024
- **Layout**: single binary crate — source lives under `src/`; the binary
  is `grim`, the crate/package is `grimoire`. No lib/CLI split, no workspace.
- **Default registry**: configurable via `GRIM_DEFAULT_REGISTRY`
- **Testing**: pytest acceptance tests under `test/` against a real OCI
  registry

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

**Who must check** — every agent at product level re-reads this file when
work could shift positioning: `worker-researcher` after evaluating a
library/tool; `worker-architect` after an ADR or design spec;
`worker-doc-writer` after user-guide edits; `worker-builder` /
`worker-reviewer` if implementation exposes a capability gap or breaks a
stated principle.

**Validation** — `/meta-maintain-config refresh` spot-checks this file
against current CLI help, source code, and recent ADRs.
