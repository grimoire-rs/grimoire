---
name: grim-authoring
description: Author, validate, and package grim-publishable artifacts — skill directories, rule files, agent definitions, MCP server descriptors, and bundle TOMLs. Use when creating or editing an artifact for grim build or grim release; when choosing frontmatter or catalog metadata fields; when adding claude, opencode, or copilot vendor keys; or when grim build fails validation with exit code 65.
license: Apache-2.0
compatibility: grim>=0.9
metadata:
  summary: Deep authoring guide for grim skill, rule, agent, mcp, and bundle artifacts
  keywords: grim,grimoire,authoring,frontmatter,validation,vendor-metadata,skill,rule,agent,mcp,bundle,packaging
  repository: https://github.com/grimoire-rs/grimoire
---

# Grim Artifact Authoring

Grim publishes five artifact kinds to OCI registries. Each has its own
source shape, frontmatter schema, and validation gates. This root file
holds the invariants that apply to every kind; per-kind depth lives in
`references/`, loaded via the routing table below.

## The Five Kinds

`grim build` and `grim release` infer the kind from the path — except
agents (always `--kind agent`, or they silently pack as rules) and MCP
servers (always `--kind mcp`, or the `.toml` is treated as a bundle).

| Kind | Source shape | Inference | Installs as |
|---|---|---|---|
| Skill | Directory with a `SKILL.md` index | directory → skill | Directory tree under the client's `skills/` dir |
| Rule | Single `.md` file | `.md` → rule | `rules/<name>.md`, per-client transform |
| Rule + support dir | `<name>.md` + sibling `<name>/` dir | sibling dir auto-discovered | Index file + `rules/<name>/…` side by side |
| Agent | Single `.md`, frontmatter required | **never — `--kind agent` mandatory** | One agent file per client, per-client render |
| MCP server | `.toml` descriptor with a `[server]` table | **never — `--kind mcp` mandatory** | Entry in each client's MCP config file, per-client render |
| Bundle | `.toml` member list | `.toml` → bundle | Never materializes — expands to its members |

## Universal Invariants

- Names are `[a-z0-9]` runs joined by single hyphens or periods
  (`[a-z0-9]+([.-][a-z0-9]+)*`) — non-empty, ≤ 64 chars, no leading or
  trailing separator, no adjacent separators (`a--b` and `a..b` are
  invalid). Periods are a grim superset of the Agent Skills standard
  (`[a-z0-9-]`) — prefer hyphens when portability to strict-standard
  tooling matters.
- A skill's `name` must equal its directory name; an agent's `name` must
  equal its file stem. Rule names come from the file stem and obey the
  same character rules. Bundle and MCP names also come from the file stem
  but are not charset-validated at build; bundle *member* names are
  validated against the same rules at resolve time.
- Any violation of the validated names fails `grim build`/`grim release`
  with exit code 65.
- Unknown top-level frontmatter keys are *preserved* round-trip (forward
  compatibility) — never rejected, so a typo'd optional key is silent.

## The Metadata-Location Asymmetry

Where catalog metadata (`summary`, `keywords`, `repository`, `deprecated`,
`replaced-by`) is authored differs by kind. This is the #1 authoring
confusion — misplaced keys are not errors, they just silently never reach
the catalog:

| Kind | `summary` / `keywords` / `repository` / `deprecated` / `replaced-by` live… |
|---|---|
| Skill | inside the `metadata:` map of `SKILL.md` frontmatter |
| Agent | inside the `metadata:` map of the agent frontmatter |
| Rule | at the **top level** of the rule frontmatter (not in `metadata`) |
| MCP server | as **top-level TOML keys**, above the `[server]` table (`replaced-by` not read for MCP) |
| Bundle | as **top-level TOML keys**, above the member tables |

In every kind, `keywords` is one comma-separated string and `repository`
must be an `https://` URL (anything else fails the release with 65). The
`deprecated` notice obeys the same per-kind location; an
empty or whitespace-only value means *not* deprecated and emits no
annotation. `replaced-by` names the successor artifact, authored
independently of `deprecated`; its value must parse as a reference or the
release fails with 65 — detail in [Publishing][publishing].

## Companion: Content Craft

This skill covers grim **packaging and validation** only — including opt-in
git provenance at build/release time (`--git`); confirm flags with
`grim release --help`. For the craft of
the content itself — progressive disclosure, context budgets, description
triggering, choosing skill vs rule vs agent — read the companion skill
`ai-config-authoring` at
[`../ai-config-authoring/SKILL.md`](../ai-config-authoring/SKILL.md);
both ship together in the `grim-essentials` bundle. When creating a new
artifact from scratch, read it FIRST — write good content, then package
it here. If that file is missing, install it by identifier:

```sh
grim add ghcr.io/grimoire-rs/skills/ai-config-authoring:0   # installs by default
# fresh project (no grimoire.toml yet): run `grim init` first
```

## The Local Dev Loop

Iterate on an artifact **before** its first release with local path
sources — no registry round-trip:

- `grim install <path>` — **dev-install**: renders the working tree into
  the clients without declaring anything (`grimoire.toml` and
  `grimoire.lock` stay untouched). The record is marked `dev` in
  `grim status`, refreshed by `grim update`, removed by `grim uninstall`.
- `grim add <path>` — declares the local path in the config and pins it
  by content hash, like any other source.

A path is anything starting `./` or `../`, or absolute. Both commands
cover **skills, rules, and agents** only; kind is inferred from the
path's shape exactly as `grim build` infers it (directory → skill, bare
`.md` → rule, `--kind agent` for agents). A local *bundle* is declared
directly in the config's `[bundles]` table instead (`grim add --kind
bundle <path>` refuses with a hint); its members must be registry
references — a local bundle has no registry identity to resolve a
relative member against. Typical loop: edit → `grim build <path>`
(validation) → `grim install <path>` (see it in a real client) →
repeat → release. Confirm flags with `grim install --help`.

## Routing Table

| Read… | …when |
|---|---|
| [references/skill-spec.md](references/skill-spec.md) | Authoring a skill directory or its `SKILL.md` frontmatter |
| [references/rule-spec.md](references/rule-spec.md) | Authoring a rule file, its globs, or a support directory |
| [references/agent-spec.md](references/agent-spec.md) | Authoring an agent definition or its vendor overrides |
| [references/mcp-spec.md](references/mcp-spec.md) | Authoring an MCP server descriptor or its env references |
| [references/bundle-spec.md](references/bundle-spec.md) | Authoring a bundle TOML or choosing pinning strategy |
| [references/vendor-metadata.md](references/vendor-metadata.md) | Adding `claude.*` / `opencode.*` / `copilot.*` keys |
| [references/release-checklist.md](references/release-checklist.md) | Before `grim release`/`grim publish`, batch manifests, description companions, or triaging an exit-65 failure |
| [references/updating.md](references/updating.md) | Maintaining this skill package itself |

## Schema Authority

This skill teaches the craft and the pitfalls; the authoritative schema
reference is the Grimoire docs site. When a field table here feels
incomplete, the docs page is the source of truth:
[Artifact Reference][artifacts] · [Vendor-Specific Metadata][vendor] ·
[Publishing][publishing] · [Agent Artifacts][agents]. For the TOML
surfaces, `grim schema --kind <config|publish|lock>` prints the JSON
Schema generated from grim's own parsers — bind it in your editor to
catch manifest typos before any command runs.

## Verify Before Acting

`grim build <path>` validates without pushing — run it after every edit;
its output is ground truth for the grim version actually installed. On
any conflict between this skill and `grim build` output or `grim --help`,
trust the tool. Treat this skill as the map, not the territory.

---

Verified against grim 0.9.0.

[artifacts]: https://grimoire.rs/artifacts.html
[vendor]: https://grimoire.rs/vendor-metadata.html
[publishing]: https://grimoire.rs/publishing.html
[agents]: https://grimoire.rs/agents.html
