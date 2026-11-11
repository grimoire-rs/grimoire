# Research: Artifact metadata placement inventory (frontmatter vs TOML descriptor)

## Metadata

**Date:** 2026-11-10
**Domain:** packaging
**Triggered by:** design discussion `.agents/discussions/artifact-metadata-placement.md`: where the line sits between "declared in the content file's frontmatter" and "declared in a TOML descriptor", for the unreleased `requires` declaration
**Expires:** 2027-05-10

Read vs inferred: every row marked `[read]` cites code I opened. `[inferred]` is reasoning from a nearby comment or test, no execution. No build or test was run.
No recommendation is given.

## Direct Answer

Two source shapes exist today.

- **Markdown kinds (skill, rule, agent):** the content file is also the descriptor. Catalog fields are in frontmatter, but the three kinds spell the same fields differently (see "Same concept, different spelling").
- **TOML kinds (mcp, settings, bundle, hook):** a separate descriptor. mcp, settings and bundle carry catalog fields top-level; hook carries none.

`requires` placement already differs along that line: `grim.requires.<table>.<name>` in frontmatter `metadata` for the Markdown kinds, `[requires.<table>]` TOML tables for mcp/hook/settings, `[bundles]` and kind tables for bundle (`src/oci/requires_decl.rs:5-12`, `docs/src/content/docs/artifacts.md:692-698`).

Everything declared by `requires` reaches the consumer through a separate OCI layer, not through the content layer (`src/command/release.rs:708-721`, `src/oci/manifest.rs:81-120`).

## 1. Field table

Legend. Consumer: G = grim only, C = AI client at runtime, G+C = both.
Wire: CL = content layer, JL = JSON layer blob (mcp/settings), RL = separate requires layer (`application/vnd.grimoire.bundle.v1+json`, `src/oci/bundle.rs:26`), ANN = manifest annotation.
`fm.` is frontmatter, `fm.metadata.` is the `metadata` map.

### Skill (`SKILL.md`; struct `src/skill/skill_frontmatter.rs:36-74`)

| Field | Source spelling | Consumer | Wire | Installed fate | Evidence |
|---|---|---|---|---|---|
| name | `fm.name` (required) | G+C | CL verbatim; ANN `image.title` | kept; rewritten only for rebind (`render.rs:666`) | skill_frontmatter.rs:38; annotations.rs:412 |
| description | `fm.description` (required) | G+C | CL; ANN `image.description` | kept | skill_frontmatter.rs:40; annotations.rs:413-416 |
| license | `fm.license` top-level | G (annotation); C ignores [inferred] | CL; ANN `image.licenses` | kept (serialized back when rendered) | skill_frontmatter.rs:44; annotations.rs:425 |
| compatibility | `fm.compatibility` top-level | G (annotation, warn on empty/>500) | CL; ANN `com.grimoire.compatibility` (skill only) | kept | skill_frontmatter.rs:47; annotations.rs:225-231,479; render.rs:765-790 |
| allowed-tools | `fm.allowed-tools` top-level | C (grim only carries) | CL only, no annotation | kept | skill_frontmatter.rs:62-63 |
| summary | `fm.metadata.summary` | G | CL; ANN `com.grimoire.summary` | kept (plain metadata) | annotations.rs:474-476 |
| keywords | `fm.metadata.keywords` | G | CL; ANN `com.grimoire.keywords` | kept | annotations.rs:471-473 |
| repository | `fm.metadata.repository` | G (HTTPS gate at build) | CL; ANN `image.source` (+ `url`, `documentation` fallbacks) | kept | annotations.rs:429-436,151-153; build.rs:292 |
| authors, vendor, homepage, documentation | `fm.metadata.<key>` | G | CL; ANN `image.authors/vendor/url/documentation` | kept | annotations.rs:439-466 |
| deprecated, replaced-by | `fm.metadata.<key>` | G | CL; ANN `com.grimoire.deprecated/replaced-by` | kept | annotations.rs:482-487 |
| `<vendor>.<field>` (e.g. `claude.*`) | `fm.metadata.<vendor>.<field>` | G reads; lifts to native key for C | CL verbatim; not annotated | lifted into native top-level keys for the matching client, dropped for other clients, own unknown key dropped with a warning | render.rs:266-318,334-356,568-600 |
| `grim.requires.<table>.<name>` | `fm.metadata`, flattened | G | CL verbatim; also RL (parsed, sorted) | kept (see section 3) | requires_decl.rs:19,114-129; build.rs:301-305; release.rs:276,306 |
| any other key | top-level, `extra` | unknown | CL verbatim | kept | skill_frontmatter.rs:71-73 |

### Rule (`<name>.md`, frontmatter optional; struct `src/skill/rule_frontmatter.rs:27-41`)

| Field | Source spelling | Consumer | Wire | Installed fate | Evidence |
|---|---|---|---|---|---|
| name | none; file stem | G | ANN `image.title` | n/a | annotations.rs:507-511; build.rs:326-329 |
| description | none; derived from first heading/paragraph of body | G | ANN `image.description` | n/a | rule_frontmatter.rs:77-90; annotations.rs:510 |
| paths | `fm.paths` | C (grim maps per client) | CL | Claude: verbatim. Cursor: `globs` string + `alwaysApply`. Copilot: `applyTo`. | rule_frontmatter.rs:28-30; vendor_cursor.rs:239-245; vendor_copilot.rs:498-520 |
| license | `fm.license` top-level, via `extra` | G | CL; ANN `image.licenses` | kept for Claude | annotations.rs:515-518 |
| summary, keywords | top-level via `extra`, **string only** (a YAML list is ignored) | G | CL; ANN | kept for Claude | annotations.rs:546-551,903-905; test annotations.rs:1209-1223 |
| repository, authors, vendor, homepage, documentation, deprecated, replaced-by | top-level via `extra` | G | CL; ANN | kept for Claude | annotations.rs:519-563 |
| `<vendor>.<field>` | `fm.metadata.<vendor>.<field>` | G reads, C via lift | CL | Claude: re-rendered only if a tool key exists, else verbatim | render.rs:360-372,538-566; vendor_claude.rs:518-529 |
| `grim.requires.<table>.<name>` | **`fm.metadata`** (not top-level) | G | CL; also RL | see section 3 | build.rs:370-376; release.rs:665-667 |

### Agent (`<name>.md`, frontmatter required; struct `src/skill/agent_frontmatter.rs:39-64`)

| Field | Source spelling | Consumer | Wire | Installed fate | Evidence |
|---|---|---|---|---|---|
| name, description | `fm.name`, `fm.description` (required) | G+C | CL; ANN | name/description kept in every vendor format | agent_frontmatter.rs:41-43; annotations.rs:574-588 |
| model, tools | `fm.model`, `fm.tools` top-level | C | CL | passed through, `tools` projected per client (list vs string); `<vendor>.model/tools` overrides | agent_frontmatter.rs:45-53; vendor_copilot.rs:540-557 |
| license | **`fm.metadata.license`** | G | CL; ANN `image.licenses` | kept only on Claude/verbatim path | annotations.rs:590-597 |
| summary, keywords, repository, authors, vendor, homepage, documentation, deprecated, replaced-by | `fm.metadata.<key>` | G | CL; ANN | as for skill | annotations.rs:599-650 |
| `<vendor>.<field>` | `fm.metadata` | G reads, C via lift | CL | per-vendor `agent_index` | render.rs:447-536 |
| `grim.requires.*` | `fm.metadata`, flattened | G | CL; also RL | see section 3 | build.rs:345-358 |

### MCP server (`mcp/<name>.toml`; struct `src/oci/mcp.rs:171-213`, `deny_unknown_fields`)

| Field | Source spelling | Consumer | Wire | Installed fate | Evidence |
|---|---|---|---|---|---|
| name | none; file stem (and config binding) | G | ANN `image.title` | registered as the key in client MCP config [inferred] | mcp-servers.md "descriptor name is the file stem" (artifacts.md:506-508) |
| description | top-level TOML, required | G | JL; ANN | not written to client config [inferred] | mcp.rs:173-175 |
| summary, keywords, license, repository, deprecated, replaced-by, authors, vendor, homepage, documentation | top-level TOML | G | JL (serialized struct) **and** ANN | not written to client config [inferred] | mcp.rs:176-210; annotations.rs:728-780 |
| `[server]` | TOML table | C (registered in client config) | JL | translated per client | mcp.rs:211-212 |
| `[requires.<table>]` | TOML tables | G | **split off before parse, not in JL**; RL | not installed | mcp.rs:298-310; build.rs test "leaves descriptor bytes alone" (build.rs:863-878); artifacts.md:720-726 |
| vendor-namespaced keys | none (unknown field is a hard parse error) | n/a | n/a | n/a | mcp.rs:170 `deny_unknown_fields`; artifacts.md:510-512 |

### Hook (`hook.toml` inside a payload dir; struct `src/oci/hook.rs:395-417`, `deny_unknown_fields`)

| Field | Source spelling | Consumer | Wire | Installed fate | Evidence |
|---|---|---|---|---|---|
| schema, name, description | top-level TOML | G | CL (the whole `hook.toml` is packed); ANN title/description | name = directory stem | hook.rs:397-405; annotations.rs:855-870 |
| summary, keywords, repository, deprecated, replaced-by | **not accepted** (unknown key = parse error) | n/a | only `publish.toml [metadata]` defaults fill license/repository/authors/vendor/homepage/documentation | n/a | annotations.rs:845-896; hook.rs:396 |
| `[[hooks]]` entries (event, tier, matcher, argv/command, timeout, payload, `<vendor>.*`) | TOML array of tables | G dispatches; C triggers | CL | registered in dispatch table and launcher | hook.rs:329-380 |
| `[requires.<table>]` | TOML tables, field `requires` | G | **stays in CL verbatim** (not split off); also RL | the consumer parser accepts and ignores it | hook.rs:410-416; skill_package.rs:203-211 (`hook.toml` never ignored when packing); release.rs:681 |

### Settings (`settings.toml`; `src/oci/settings.rs:60-95`)

| Field | Source spelling | Consumer | Wire | Installed fate | Evidence |
|---|---|---|---|---|---|
| description (required), summary, keywords, license, repository, deprecated, replaced-by, authors, vendor, homepage, documentation | top-level TOML | G | JL **and** ANN | not written into client settings [inferred] | settings.rs:60-80,150-179; annotations.rs:791-838 |
| `<vendor>` tables (the settings themselves) | top-level tables keyed by vendor name | C | JL | merged into the client's settings file | settings.rs:94,166-168 |
| `[requires.<table>]` | TOML tables | G | split off, **not in JL**; RL | not installed | settings.rs:118-129; artifacts.md:697 |

### Bundle (`<name>.toml`; `BundleSource`/`BundleMetadata`, `src/config/project_config.rs:743-824`)

| Field | Source spelling | Consumer | Wire | Installed fate | Evidence |
|---|---|---|---|---|---|
| name | file stem | G | ANN title | never materialized | build.rs:503-510,896-898 |
| description (overrides default), summary, keywords, license, repository, deprecated, replaced-by, authors, vendor, homepage, documentation | top-level TOML | G | ANN only | n/a (bundle never installs) | project_config.rs:746-777; annotations.rs:664-720 |
| members | `[skills]`, `[rules]`, `[agents]`, `[hooks]`, `[mcp]`, `[settings]`, `[bundles]` tables of name = ref | G | JL (the bundle layer is the members document) | expanded into the lock | project_config.rs:786-821; bundle.rs:1-25 |

## 2. Which fields are meaningful for which kinds

| Field | skill | rule | agent | mcp | hook | settings | bundle | Evidence |
|---|---|---|---|---|---|---|---|---|
| name | fm | stem | fm | stem | TOML, must equal stem | stem | stem | see tables |
| description | fm, required | derived from body | fm, required | TOML, required | TOML, required | TOML, required | optional override | see tables |
| license, repository, deprecated, replaced-by, authors, vendor, homepage, documentation | yes | yes | yes | yes | **no** (publish.toml defaults only) | yes | yes | annotations.rs:845-896 |
| summary, keywords | yes | yes | yes | yes | **no** | yes | yes | annotations.rs:851 |
| compatibility | **skill only** (own annotation) | no | no | no | no | no | no | annotations.rs:225-231 |
| allowed-tools | skill only | no | no | no | no | no | no | skill_frontmatter.rs:62 |
| model, tools | no | no | agent only | no | no | no | no | agent_frontmatter.rs:45-53 |
| paths | no | rule only | no | no | no | no | no | rule_frontmatter.rs:28 |
| `<vendor>.<field>` keys | metadata map | metadata map | metadata map | no (hard parse error) | `<vendor>.event` and similar per-entry override inside `[[hooks]]` | vendor top-level tables are the payload, not overrides | no | render.rs:266-318; hook.rs:334-336; settings.rs:94 |
| requires | metadata keys | metadata keys | metadata keys | `[requires.*]` | `[requires.*]` | `[requires.*]` | kind tables + `[bundles]` | requires_decl.rs:5-12 |

## 3. `grim.requires.*` in the file a client finally reads

Finding: they are **present** whenever the install is a verbatim copy, and **present in plain metadata** when the skill is re-rendered. They are absent only where an agent or rule format is rebuilt from native keys.

Code path [read]:

1. `grim` is not in `KNOWN_NAMESPACES`. That list is `ClientTarget::ALL` minus `agents`, mapped to vendor names (`render.rs:67-73`); the vendor names are cline, amp, claude, openclaw, opencode, codex, kilo, goose, gemini, qoder, antigravity, cursor, zed, kiro, copilot, junie, warp, droid. `split_namespaced("grim.requires.x")` returns `None` (`render.rs:166-169`), so the key is plain metadata. I grepped the `vendor_*.rs` names and found no vendor named `grim`.
2. Skill: `render_skill_doc` returns `None` (verbatim bytes) when no tool-namespaced key exists (`render.rs:568-598`, `has_tool_namespaced_metadata` at `render.rs:149-151`). When a `claude.*` key exists, `partition_metadata` puts every non-namespaced key into `plain` (`render.rs:277-282`) and `project_skill` re-serializes `plain` metadata (`render.rs:334-356`). So `grim.requires.*` survives in both cases. Every vendor `skill_index` calls `render_skill_doc` or the universal variant (grep of `vendor_*.rs`: claude:515, copilot:495, cursor:220, codex:513, gemini:327 and others; `vendor_agents.rs:113`, `vendor_goose.rs:332` use the universal render, which also keeps plain keys, `render.rs:620-664`).
3. Rule, Claude: `render_rule_canonical` returns `None` (verbatim) when no tool key exists, else re-serializes `cleaned` with plain metadata kept (`render.rs:538-566`; `vendor_claude.rs:518-529`). So kept. Rule, Cursor: frontmatter is rebuilt as `globs`/`alwaysApply` only, `metadata` not emitted (`vendor_cursor.rs:229-245`). Rule, Copilot: `applyTo` plus lifted keys only (`vendor_copilot.rs:504-520`). So **stripped** on those two. Other vendors' `rule_index` that return `Ok(None)` (codex:524, amp:254, cline:237, gemini:337) I did not trace further: [inferred] the client has no rule surface or takes verbatim; not verified.
4. Agent, Claude: `render_agent_canonical`, verbatim or plain metadata kept (`render.rs:471-505`; `vendor_claude.rs:531-540`). Agent, Copilot (and the other `agent_index` impls I listed: opencode, cursor, gemini, antigravity, junie, droid, goose, kilo, codex TOML): only `name`, `description`, `model`, `tools` plus lifted vendor keys are emitted, so **stripped** (`vendor_copilot.rs:530-562`; same shape at `vendor_codex.rs:537-563`).
5. Docs say so in plain words: "The keys ride in the packed skill, rule or agent as ordinary metadata, which no client reads" (`artifacts.md:728-731`). That statement is about the packed layer, not the installed file.

Content layer: the source file is tarred as-is (`pack_skill_dir`/`pack_rule_file`/`pack_agent_file`, `skill_package.rs:199-201,283-308,347-366`), so the keys ride verbatim in the content layer and also form the RL.

`--pin` consequence [inferred from `release.rs:588-600,606` and `requires_layer`]: with `--pin` the requires layer carries digest-pinned ids while the frontmatter keys in the content layer keep the authored floating text. A consumer reading the published artifact uses the RL (`hook.rs:410-414` comment; `manifest.rs:81-120`); a path-sourced install reads the frontmatter (`resolver.rs:999-1031`).

## 4. Same concept, different spelling

1. **summary/keywords/repository/license/deprecated/replaced-by/authors/vendor/homepage/documentation:** skill and agent in `metadata.<key>`; rule top-level; mcp, settings, bundle top-level TOML; hook not available (`annotations.rs:471-487,546-563,599-650`; `artifacts.md:380-381,452`).
2. **license:** skill top-level `license`; agent `metadata.license`; rule top-level `license`. A top-level `license` in an agent lands in `extra` and is not annotated (`annotations.rs:425,590-597,515`).
3. **requires, within the Markdown kinds:** rule `summary`/`keywords` are top-level, but rule `requires` is in `metadata` (`build.rs:370-376` reads `fm.metadata`), so a rule has catalog keys top-level and `requires` inside `metadata`.
4. **requires across kinds:** flattened dotted key `grim.requires.<table>.<name>` (skill/rule/agent) vs `[requires.<table>]` table (mcp, hook, settings) vs `[bundles]`/kind tables as the membership itself (bundle) (`requires_decl.rs:5-12`).
5. **requires and the descriptor bytes:** mcp and settings split `requires` off before packing, so it is not in their JL (`mcp.rs:298-310`, `settings.rs:118-129`); hook leaves it in the packed `hook.toml` (`hook.rs:410-416`, `skill_package.rs:203-211`); skill/rule/agent leave it in the packed file.
6. **Vendor extension spelling:** `<vendor>.<field>` metadata key (skill/rule/agent) vs per-entry `<vendor>.event` style override key in `[[hooks]]` (hook) vs top-level vendor table (settings) vs not allowed (mcp) (`hook.rs:334-336`, `settings.rs:94`, `artifacts.md:510-512`).
7. **name:** frontmatter `name` (skill, agent), file stem (rule, mcp, settings, bundle), TOML `name` plus stem rule (hook).

## 5. Published spelling of `requires`

- `docs/src/content/docs/artifacts.md:692-731`: the full table above, both examples (`metadata: grim.requires.skills.gws-shared`, `[requires.skills]`).
- `docs/src/content/docs/guides/dependencies.md:70,83,204,214`: `grim.requires.skills.*` and `grim.requires.mcp.gdrive` examples.
- `docs/src/content/docs/publishing.md:942-976`: dependency publish flow; no frontmatter spelling checked beyond a link back to artifacts.md.
- `docs/src/content/docs/settings.md:126`: `requires` kept out of the settings layer.
- `catalog/skills/grim-authoring/SKILL.md:71-72`: both spellings in the authoring skill (drift-review surface).
- `test/manual/catalog/agents/triage-bot.md:9-10`: example agent.
- Schemas: `docs/public/schemas/grim-mcp.schema.json:222-240` and `grim-settings.schema.json:2160-2170` publish `requires` as a table-of-tables property. `grimoire-lock.schema.json:203-205` publishes the *lock's* `requires` edges. `grim-publish.schema.json:306` mentions `requires` only under `pin`. **No schema covers skill/rule/agent frontmatter or `hook.toml`** (only five schemas exist in `docs/public/schemas/`).
- Not in any changelog: `CHANGELOG.md` and `docs/src/content/docs/changelog.md` contain no `requires` dependency entry. `git tag --contains e51bad72` (the commit adding dependencies) printed nothing, so the declaration is in no tag [read].

## 6. negative:

- negative: no top-level `requires` key is parsed for skill/rule/agent frontmatter; `from_metadata` is the only reader (`requires_decl.rs:114-129`). A top-level `requires:` would sit in `extra` and be ignored [inferred].
- negative: `grim.*` keys other than `grim.requires.*` do not exist; grep of `src/**/*.rs` for a `"grim.` literal found nothing outside `requires_decl.rs` and a TUI test (`src/tui/app.rs:12159`).
- negative: no JSON schema or docs page documents the skill/rule/agent frontmatter fields as a schema; only the prose table in `artifacts.md`.
- negative: hook has no `summary`/`keywords`/`repository`; the Decision 6 list says hook uses `[requires.*]`, which holds, but the list does not say hook is the one TOML kind where `requires` stays in the packed file.
- contradicts the "already known" list: Decision 6 groups mcp, hook, settings together as "split off". Code splits it off only for mcp and settings; hook keeps it in the content layer (`hook.rs:410-416` comment: "the consumer parser accepts and ignores it").
- contradicts the docs: `artifacts.md:375` says rule `keywords` accepts "string or list (a YAML list is comma-joined)"; code and test ignore a list (`annotations.rs:903-905,1209-1223`).
- docs drift: `artifacts.md:177-184` says "three plain keys are read by grim" (`summary`, `keywords`, `author`) and `author` is "read by nothing". Code reads also `repository`, `authors`, `vendor`, `homepage`, `documentation`, `deprecated`, `replaced-by` from skill metadata (`annotations.rs:429-487`). Note the singular `author` is not the read key `authors`.
- not found: any installed-file path that adds or removes `grim.requires.*` on purpose; they are simply plain metadata. Whether stripping in the rebuilt agent/rule formats is intended is not stated anywhere I read.
- not verified: mcp/settings installed output (I did not read the client config writers); marked [inferred] in the tables.

## 7. leads:

- lead: `--pin` makes the frontmatter `requires` (floating, in the content layer) and the published requires layer (pinned) diverge; which one a consumer trusts is `resolver.rs:999-1031` vs the manifest layout reader, worth a one-line check of precedence.
- lead: a rule keeps catalog keys top-level but `requires` in `metadata`; an author must know both (`build.rs:370-376`), and a rule whose catalog keys moved would change nothing for `requires`.
- lead: `hook.toml` is the only descriptor format already published to third-party tooling as TOML 1.0 (`hook.rs:386-394`); adding keys there is bounded by `deny_unknown_fields`, so a new top-level key is a parser change on older grims (stability.md additive-field rules apply).

## Sources

| Source | Type | Date | Relevance |
|---|---|---|---|
| `src/oci/requires_decl.rs` | code | 2026-11-10 | requires grammar and prefix |
| `src/oci/annotations.rs` | code | 2026-11-10 | field-to-annotation map per kind |
| `src/install/render.rs`, `src/install/vendor_*.rs` | code | 2026-11-10 | installed-output fate |
| `src/oci/{mcp,hook,settings,bundle,manifest}.rs`, `src/command/{build,release}.rs` | code | 2026-11-10 | wire layout and splitting |
| `docs/src/content/docs/artifacts.md` | docs | 2026-11-10 | published field tables and requires spelling |
| `docs/public/schemas/*.json` | schemas | 2026-11-10 | what is published as schema |
