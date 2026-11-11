# Research: Metadata placement prior art (dependencies in-file vs manifest vs per-kind)

<!-- Neutral evidence report for a design discussion. No recommendation by design. -->

## Metadata

**Date:** 2026-11-10
**Domain:** packaging
**Triggered by:** Design discussion on where package-manager metadata (above all the dependency declaration) lives relative to artifact content, for single-file kinds (skills, rules, agents) and descriptor kinds (MCP, hooks, settings, bundles).
**Expires:** 2027-05-10 (the AI-config ecosystem moves fast; Claude Code and APM pages below changed within weeks)

Provenance tags: **[P]** read from primary source on 2026-11-10 via fetch; **[S]** secondary source; **[I]** inferred or from memory, not fetched this session.

## Direct Answer

No recommendation. Evidence summary:

- Placement (b) (separate manifest) is the majority for systems that have a real dependency concept: Claude Code plugins, APM, Helm, VS Code, Go, Ansible collections. Single-file artifacts inside those systems carry no dependency data of their own.
- Placement (a) (in-file) has a strong precedent where the single file must travel alone (PEP 723) and in one third-party skill tool (sklock `requires`). No first-party AI-config system defines a dependency key in file frontmatter.
- Placement (c) (per kind, one grammar) occurs de facto inside Claude Code plugins: `plugin.json` for the package, frontmatter for content behaviour, `.mcp.json`/`hooks.json` for descriptors. Dependencies are declared only at the package level, in the manifest.
- The Agent Skills spec has no dependency field. `metadata` is the only extension slot, restricted to string-to-string.

## Findings by placement

### (b) Separate manifest next to the content

1. **Claude Code plugins.** Dependencies live in `plugin.json` `dependencies` (strings or `{name, marketplace, version}`), semver ranges resolved against git tags `<plugin>--v<version>`. Entry may also carry `dependencies` in `marketplace.json`. [P] https://code.claude.com/docs/en/plugins-reference , https://code.claude.com/docs/en/plugins/dependencies.md
   - Manifest is optional; without it components load from the standard layout and the name comes from the marketplace entry or directory. A plugin with `SKILL.md` at root and no `skills/` loads as a single skill. [P] plugins-reference
   - A bundle is a plugin with only `name` + `dependencies`: "A plugin manifest needs only `name`, so this is a valid plugin". [P] dependencies.md
   - Unknown top-level `plugin.json` keys are stripped with a validate warning; unknown keys in strict objects (`userConfig`, `channels`, `lspServers`, `monitors`) are errors. `metadata` is free-form and not read. [P]
   - Entry-vs-manifest precedence is a documented pain surface: `version` in manifest overrides entry; hooks entries replace per-event; `strict:false` plus entry components is a "conflicting manifests" load failure; entry name vs manifest name mismatch fails install. [P] plugins-reference, plugin-marketplaces
   - Unit of dependency is the plugin, never an individual skill. [P] (no skill-level dependency key in the docs read)
   - Rationale: not stated as a rationale for placement; stated purpose is protecting dependents from breaking updates. [P]
2. **APM (Microsoft Agent Package Manager).** `apm.yml` holds `dependencies: {apm, mcp, lsp}` and `devDependencies`; `apm.lock.yaml` pins. Single files (`.prompt.md`, `.instructions.md`, `.agent.md`) can be installed as dependencies but "carry no dependency data of their own"; "Single-file packages contain only that file". Directory shape resolved by probing for `apm.yml` first. Legacy `.collection.yml` removed in favour of `apm.yml` `dependencies:`. Unknown top-level keys must be preserved on rewrite; vendor keys `x-*`. [P] https://microsoft.github.io/apm/reference/manifest-schema/ , https://microsoft.github.io/apm/specs/openapm-v01/ (first 100k chars of a 328k page read)
   - Stated rationale for manifest placement: none found; spec cites interoperability of the on-disk format. [P] (absence)
   - Single-file artifacts are explicitly a factor: the manifest is addressed at them from outside via `virtual_path`. [P]
3. **Helm.** `Chart.yaml` `dependencies` (apiVersion v2); v1 charts used a separate `requirements.yaml`. Page gives no rationale for the move. [P] https://helm.sh/docs/topics/charts/ . Note this is a move between two manifests, not in-content to manifest.
4. **VS Code.** `extensionDependencies` and `extensionPack` in `package.json`. [P] https://code.visualstudio.com/api/references/extension-manifest . Packs and dependencies are both manifest arrays of IDs; a pack is a manifest with no behaviour of its own (parallels Claude bundle plugin).
5. **Go.** `go.mod` `require`; import statements carry no versions; "the module graph defines the meaning of import statements". Version chosen once per build rather than per file. [P] https://go.dev/ref/mod . Direct evidence of why per-file version declarations were avoided: one version per module graph.
6. **Ansible.** Role dependencies in `meta/main.yml` (a manifest inside the role directory); collection dependencies in `galaxy.yml`. The page fetched did not describe either in detail. [P-partial] https://docs.ansible.com/ansible/latest/galaxy/dev_guide.html ; remainder [I].
7. **Kubernetes.** Annotations hold tool-read, non-identifying metadata alongside the spec; the page argues against storing it in an external database because shared tooling is harder. Keys prefixed, values strings, 256 KiB cap. [P] https://kubernetes.io/docs/concepts/overview/working-with-objects/annotations/ . Closest analogue to Agent Skills `metadata` (string map, namespaced-by-convention). It is in-object but in a reserved side channel, not in the content body.
8. **MCP registry `server.json`.** Single descriptor file; `packages[]`, `remotes[]`; no field for dependencies on other servers; `_meta` for publisher data under reverse-DNS key. [P] https://raw.githubusercontent.com/modelcontextprotocol/registry/main/docs/reference/server-json/generic-server-json.md
9. **MCPB `manifest.json`.** Manifest is the package; no dependency declaration across bundles; runtime deps must be bundled (`node_modules`, bundled Python) or declared in `pyproject.toml` for UV type; optional `compatibility`; `_meta` reverse-DNS. Unknown-field handling not specified. [P] https://raw.githubusercontent.com/modelcontextprotocol/mcpb/main/MANIFEST.md
10. **Ktesio `skills.json`** (project-level manifest declaring skill dependencies). Found only in a search snippet; page did not resolve. [S, unverified] https://ktesio-docs.pages.dev/manifest

### (a) Inside the content file

1. **PEP 723 inline script metadata.** Explicit placement-(a) precedent with published rationale. Single-file scripts "may live as a single file forever"; metadata embedded "within the script itself, and not in an external file"; a separate requirements file "requires two files, where currently only a single file is needed", companion files are easy to separate and some sharing channels make them impractical. Rejected: comment-block requirements.txt, multi-line string, Python variables (needs a Python parser for all consumers), reusing core metadata (name/version required). [P] https://peps.python.org/pep-0723/
   - Single-file artifacts were THE factor. Mechanism is a typed TOML block in a comment, readable without a full language parser.
2. **sklock (third-party skill locker).** `requires:` list of skill IDs in SKILL.md frontmatter; `skill.lock` records content and closure hashes. Edits frontmatter via a comment-preserving parser (falls back to rewriting the block). "Other frontmatter keys ... are accepted and ignored by sklock". Rationale published: reproducibility without a lock. [P-via-fetch of README summary] https://github.com/artieax/sklock . Adoption signal not measured.
3. **Claude Code issue #20057** (request for `requires`/`depends` in SKILL.md frontmatter, name-based resolution). Opened 2026-01-22; closed "not planned" by stale bot on 2026-02-28; no maintainer response; locked. Not a rejection of the idea on merits. [P] https://github.com/anthropics/claude-code/issues/20057
4. **Claude Code subagent `skills:` frontmatter.** Subagent file lists skill names to preload (full content injected, first 32 names); missing or disabled skill is skipped with a debug-log warning. Runtime preload reference by name, in-file, with no version, no resolution, no install. Not a package dependency, but it is an in-file cross-artifact reference. [P] https://code.claude.com/docs/en/sub-agents
5. **Agent Skills `metadata` map.** Spec says `metadata` is "A map from string keys to string values", for "additional properties not defined by the Agent Skills spec", with "reasonably unique" key names recommended. Example `author`, `version`. [P] https://agentskills.io/specification . The only spec-sanctioned in-file extension point; values are strings, so a structured dependency list would need string encoding (inferred [I]).
6. **Jekyll / Hugo / Astro / Obsidian frontmatter; Deno/JSR.** Not fetched. Common knowledge [I]: frontmatter there carries content-rendering or site metadata, not package dependencies; Deno/JSR put dependency specifiers in import URLs/`deno.json` imports. Marked unverified; no claim made.

### (c) Per kind, one shared grammar

1. **Claude Code plugins as de facto (c).** Content frontmatter for behaviour (`name`, `description`, `paths`, `hooks`, `metadata` etc. for skills; separate table for subagents), `hooks/hooks.json` and `.mcp.json` as descriptors, `plugin.json` for package identity and `dependencies`. Dependencies are expressed in one grammar (`name[@marketplace]` + semver range) at one layer (manifest). The per-kind split is for content/behaviour, not for dependencies. [P] plugins-reference, skills, sub-agents
2. **APM.** Primitives are per-kind files (`.prompt.md`, `.instructions.md`, `.agent.md`, SKILL.md, MCP in manifest `dependencies.mcp`); dependency grammar is shared and sits in `apm.yml` only. [P] manifest-schema
3. **Kubernetes.** Same annotation grammar across all kinds (per-kind spec, shared `metadata.annotations`). Analogy only; objects there are all structured, no markdown kind. [P]
4. No system was found that puts the dependency declaration in frontmatter for markdown kinds and in the descriptor for descriptor kinds under one grammar. [negative, see below]

## Unknown-key behaviour (axis relevant to placement (a))

| System | Unknown top-level key | Source |
|---|---|---|
| Agent Skills spec | Spec defines six fields; `metadata` for extension. Spec text read does not say what clients do with others | [P] agentskills.io/specification |
| Claude Code, SKILL.md | Ignored without error | [P] code.claude.com/docs/en/skills |
| Claude.ai upload / Skills API / `package_skill.py` | Hard error: "Unexpected key(s) in SKILL.md frontmatter ... Allowed properties are: allowed-tools, compatibility, description, license, metadata, name" | [P] same page |
| Claude Code subagent frontmatter | Ignored; `claude --debug` logs `does not recognize or apply` (v2.1.296+) | [P] sub-agents |
| Claude Code `plugin.json` | Top-level stripped plus warning; strict sub-objects error | [P] plugins-reference |
| Cursor `.mdc` | Docs list `description`, `globs`, `alwaysApply` only; unknown-key handling not stated | [P] cursor.com/docs/context/rules |
| VS Code Copilot `.agent.md` | Docs list `name`, `description`, `tools`, `handoffs`, `user-invocable`, `disable-model-invocation`; unknown-key handling not stated; no dependency/extends field | [P] code.visualstudio.com/docs/copilot/customization/custom-agents |
| APM manifest | Preserve unknown top-level keys on rewrite; `x-` vendor extensions | [P] openapm-v01 |
| sklock | Accepts and ignores others | [P] |
| MCPB | Not stated | [P] |

Consequence for (a) [I]: a top-level `dependencies` key in a SKILL.md works in Claude Code (ignored) but is rejected on the claude.ai/API upload path; only keys under `metadata` survive every path read.

## negative:

- No first-party AI-config format (Agent Skills spec, Claude Code skills/subagents, Cursor `.mdc`, Copilot `.agent.md`) defines skill-to-skill or rule-to-rule dependencies in frontmatter. [P]
- Agent Skills spec has no dependency or `requires` field; search found no accepted proposal. The closest RFC found (agentskills/agentskills#413) concerns human interactions, not dependencies. [S] https://dev.to/rameenak/agent-skills-has-a-blind-spot-and-heres-how-to-fix-it-312b (not fetched)
- The Claude Code `requires` request (#20057) died of staleness, not of an argument against in-file placement; do not read it as evidence either way. [P]
- MCP `server.json` and MCPB have no cross-package dependency concept at all; `_meta` is the extension path. [P]
- openskills has no manifest and no dependency concept; dependencies live as prose in the SKILL.md body ("Install dependencies: pip install pypdf2"). [P] https://github.com/numman-ali/openskills
- ruler has no remote-source or dependency concept; rule files are plain markdown, no frontmatter. [P] https://github.com/intellectronica/ruler
- rulesync page fetched did not document frontmatter, sources or dependencies; unresolved. [P-absence] https://github.com/dyoshikawa/rulesync
- Terraform module-structure page has no manifest and does not cover version constraints; not usable as evidence. [P-absence]
- Ansible and Helm pages gave no published rationale for placement or the v1-to-v2 move. [P-absence]
- Counter-evidence to majority (b): PEP 723 (a), sklock (a), subagent `skills:` (a, reference only), K8s annotations (in-object side channel).
- Counter-evidence to (a) strength: unknown-key rejection on Claude upload/API path; Agent Skills `metadata` is string-to-string only; Go and APM argue one resolution point per graph/package (inferred rationale for APM, stated for Go).
- No published rationale found from any AI-config system for choosing manifest over frontmatter. APM's manifest page states none. [P-absence]
- Not read at all this session: Codex, OpenCode, Gemini agent/command file schemas; Copilot instructions/prompt-file frontmatter (page read did not cover them); Hugo/Jekyll/Astro/Obsidian; Deno/JSR; skills.sh; Ktesio.

## leads:

- Agent Skills spec repo issues/PRs (agentskills/agentskills): search `requires`, `depends`, `metadata` structured values; the only place a spec-level dependency field would land.
- Claude Code `claude plugin validate` and the skills upload path error text: confirm which frontmatter keys each distribution path rejects, since that decides whether any top-level key is safe in (a).
- npm/Cargo `package.json`/`Cargo.toml` single-file-script analogues (Cargo `-Zscript` embedded manifest, RFC 3502/3503): a second in-file precedent with published rationale alongside PEP 723.

## Sources

| Source | Type | Date read | Relevance |
|---|---|---|---|
| https://agentskills.io/specification | Spec | 2026-11-10 | Allowed frontmatter, `metadata` string map |
| https://code.claude.com/docs/en/plugins-reference | Docs | 2026-11-10 | `plugin.json` fields, `dependencies`, unknown keys, optional manifest |
| https://code.claude.com/docs/en/plugin-marketplaces | Docs | 2026-11-10 | marketplace.json entries, name rules |
| https://code.claude.com/docs/en/plugins/dependencies.md | Docs | 2026-11-10 | Dependency grammar, semver, tags, bundles |
| https://code.claude.com/docs/en/skills | Docs | 2026-11-10 | SKILL.md fields, unknown-key behaviour per path (first 100k of 119k chars) |
| https://code.claude.com/docs/en/sub-agents | Docs | 2026-11-10 | Subagent fields, `skills:` preload |
| https://github.com/anthropics/claude-code/issues/20057 | Issue | 2026-11-10 | `requires` frontmatter request, closed stale |
| https://microsoft.github.io/apm/reference/manifest-schema/ | Docs | 2026-11-10 | `apm.yml` dependencies, single-file packages |
| https://microsoft.github.io/apm/specs/openapm-v01/ | Spec | 2026-11-10 | Unknown-key preservation, `x-` keys (first 100k of 328k chars) |
| https://github.com/artieax/sklock | Repo | 2026-11-10 | In-frontmatter `requires` plus lockfile |
| https://github.com/numman-ali/openskills | Repo | 2026-11-10 | No manifest |
| https://github.com/intellectronica/ruler | Repo | 2026-11-10 | No dependencies |
| https://github.com/dyoshikawa/rulesync | Repo | 2026-11-10 | Unresolved |
| https://raw.githubusercontent.com/modelcontextprotocol/registry/main/docs/reference/server-json/generic-server-json.md | Docs | 2026-11-10 | `server.json`, `_meta` |
| https://raw.githubusercontent.com/modelcontextprotocol/mcpb/main/MANIFEST.md | Spec | 2026-11-10 | MCPB manifest |
| https://cursor.com/docs/context/rules | Docs | 2026-11-10 | `.mdc` fields |
| https://code.visualstudio.com/docs/copilot/customization/custom-agents | Docs | 2026-11-10 | `.agent.md` fields |
| https://peps.python.org/pep-0723/ | PEP | 2026-11-10 | In-file metadata rationale |
| https://helm.sh/docs/topics/charts/ | Docs | 2026-11-10 | Chart.yaml vs requirements.yaml |
| https://docs.ansible.com/ansible/latest/galaxy/dev_guide.html | Docs | 2026-11-10 | Role meta (thin) |
| https://code.visualstudio.com/api/references/extension-manifest | Docs | 2026-11-10 | `extensionDependencies`, `extensionPack` |
| https://go.dev/ref/mod | Docs | 2026-11-10 | go.mod vs imports (first 100k of 215k chars) |
| https://kubernetes.io/docs/concepts/overview/working-with-objects/annotations/ | Docs | 2026-11-10 | Annotations as side-channel metadata |
| https://developer.hashicorp.com/terraform/language/modules/develop/structure | Docs | 2026-11-10 | No manifest (thin) |
