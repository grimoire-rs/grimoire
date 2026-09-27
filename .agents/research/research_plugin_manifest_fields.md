# Research: Manifest field contracts for `grim export plugin` (Claude shape + Agent Plugins 1.0)

<!--
Technology Landscape Research
Filename: .agents/research/research_plugin_manifest_fields.md
Owner: Researcher (hex research phase)
Handoff to: Architect (/hex-architect), /hex-plan
-->

## Metadata

**Date:** 2026-09-27
**Domain:** packaging
**Triggered by:** /hex-discuss harness-native-marketplaces — field-level manifest contract for `grim export plugin`'s two output families
**Expires:** 2027-03-27

This fills the gaps the six prior artifacts left open (they establish the
landscape and per-harness matrix; this one gives the exact field tables and
the MCP mapping needed to write an emitter). Read together with
`research_plugin_support_matrix.md` (the consolidated matrix),
`research_plugin_format_compat_a.md` (full Claude/Copilot/Codex version
semantics), and `research_agent_plugins_spec_verify.md` (spec provenance).

## Direct Answer

**Claude `plugin.json`**: only `name` is required (kebab-case *recommended*,
enforced only as a warning by `claude plugin validate`; hard-rejected are
spaces, `@`, `:`, path separators, control/bidi characters — no length cap
found). `version` is a free string, never checked against semver, compared
by plain string equality against the last-installed record — so a
build-metadata-only bump (`1.2.0+a1b2c3d` → `1.2.0+def456`) **is** detected
as a new version (it's a different string), and grim's planned
`<v>+<hash>` scheme works as an update signal. Unrecognized top-level keys
are stripped with a warning, not rejected — the manifest is forgiving.

**Agent Plugins 1.0 `plugin.json`**: `$schema` (exact URL literal) and
`name` are required; `name` pattern is
`^(?!.*(?:--|\.\.))[a-z0-9](?:[a-z0-9.-]*[a-z0-9])?$`, 1–64 chars — stricter
than Claude's (lowercase-only, no uppercase, dots allowed, no consecutive
`--`/`..`). `additionalProperties: false` at root — **an unknown top-level
key is a hard schema violation**, not a stripped-with-warning field like
Claude's. `version` is optional and explicitly exempted from validation:
*"Clients MUST NOT reject a manifest solely because `version` is not valid
Semantic Versioning."* So grim's `<v>+<hash>` build-metadata string is
accepted by both families' validators — confirmed, not inferred, for
Agent Plugins; confirmed by direct doc statement for Claude.

**MCP files diverge on variable-substitution surface, not on the top-level
key** (`mcpServers` in Claude's `.mcp.json`, but Agent Plugins' `mcp.json`
wraps it under a `$schema` + `mcpServers` pair too — same key name, extra
sibling key). Claude's substitution (`${VAR}`, `${VAR:-default}`,
`${CLAUDE_PLUGIN_ROOT}`, `${CLAUDE_PLUGIN_DATA}`, `${CLAUDE_PROJECT_DIR}`)
is valid in `command`, `args`, `env`, `url`, `headers`, `headersHelper`.
Agent Plugins' substitution (`${PLUGIN_ROOT}`, `${PLUGIN_DATA}`) is valid
**only** in `args`, `env` values, and `cwd` — explicitly **not** in
`command`, `url`, `env` keys, or header names/values. That is a real
authoring constraint: an MCP descriptor whose `url` or `headers` needs a
plugin-root-relative value (unusual, but grim's `headers_helper` refinement
field is exactly this shape) has no portable expression in the Agent
Plugins family; it must be inlined literally or declined for that family.

**Skill directory naming has no cross-family lock-step rule.** Claude Code
itself does **not** require the folder name to equal the frontmatter
`name` — `name` is optional and *defaults to the directory name*; only the
separate claude.ai **web upload UI** enforces folder-name-must-match
(confirmed by `research_claude_app_install_surfaces.md`, itself flagged
unconfirmed for plugin-embedded skills). Agent Plugins 1.0 states even
less: "each immediate child directory containing a file named exactly
`SKILL.md`... is treated as one skill" with no naming rule at all,
deferring name/pattern to the separate Agent Skills spec. **Correction to
the repo's existing research**: nothing in either family's primary docs
requires directory-name/frontmatter-name equality at the *plugin-loading*
layer — grim's `strip_prefix`-style validation should treat this as
"folder name is the skill identity Claude Code uses if `name` is absent;
matching is a convention, not an enforced contract" rather than a hard
rule.

## Field Tables

### Claude `plugin.json` (`.claude-plugin/plugin.json`)

Source: [code.claude.com/docs/en/plugins-reference](https://code.claude.com/docs/en/plugins-reference), fetched 2026-09-27.

| Field | Required | Type | Validation |
|---|---|---|---|
| `name` | **yes** | string | Non-empty; no spaces, `@`, `:`, path separators, control or bidi-formatting chars. Kebab-case is a **convention** — non-kebab-case triggers only a `claude plugin validate` **warning**, plugin still loads. No documented max length. |
| `$schema` | no | string | Ignored at load time (editor autocomplete only) |
| `displayName` | no | string | Free text, any casing/spaces; not used for namespacing |
| `version` | no | string | **Not checked against semver.** A missing `version` triggers only a validator warning |
| `description` | no | string | Free text |
| `author` | no | object | `{name (required if object present), email?, url?}` |
| `homepage` | no | string | **Must parse as a URL** or the plugin fails to load |
| `repository` | no | string | Not validated |
| `license` | no | string | SPDX identifier convention, not enforced |
| `keywords` | no | array\<string\> | — |
| `metadata` | no | object | Free-form, unread by Claude Code (requires CC ≥2.1.222) |
| `defaultEnabled` | no | boolean | Default `true` |
| `dependencies` | no | array\<string\|object\> | `"name"`, `"name@marketplace"`, or `{name, marketplace, version}` |
| `settings` | no | object | Only `agent`/`subagentStatusLine` take effect; other keys dropped |
| `userConfig` | no | object | Strict-object values; unknown key inside = **hard reject** |
| `channels` | no | array\<object\> | Strict object per entry |
| `skills` | no | path\|array | Adds to (never replaces) the default `skills/` scan; `"."` allowed (CC ≥2.1.221) |
| `commands` | no | path\|array\|object | Replaces default `commands/` scan |
| `agents` | no | path\|array | `.md` files only, no directories; replaces default `agents/` scan |
| `hooks` | no | path\|object\|array | Merges with `hooks/hooks.json` |
| `mcpServers` | no | path\|object\|array | Merges with `.mcp.json`; later name wins |
| `lspServers` | no | path\|object\|array | Merges with `.lsp.json` |
| `outputStyles`, `workflows` | no | path\|array | Replaces defaults |
| `experimental.*` | no | object | `themes`, `monitors`, `evals` — shape may still change |

**Top-level unknown key**: stripped, plugin still loads, validator warns.
**Strict-object unknown key** (inside `userConfig`, `channels`,
`lspServers`, `monitors`): hard validation failure. Nothing at the manifest
root is `additionalProperties: false` the way Agent Plugins is.

### Agent Plugins 1.0 `plugin.json` (root, unqualified)

Source: [agent-plugins.org/specification](https://agent-plugins.org/specification), [plugin.schema.json](https://agent-plugins.org/schemas/1.0.0/plugin.schema.json), fetched 2026-09-27.

| Field | Required | Type | Validation |
|---|---|---|---|
| `$schema` | **yes** | string const | Must equal `"https://agent-plugins.org/schemas/1.0.0/plugin.schema.json"` exactly |
| `name` | **yes** | string | 1–64 chars, pattern `^(?!.*(?:--\|\.\.))[a-z0-9](?:[a-z0-9.-]*[a-z0-9])?$` — lowercase alphanumeric + hyphen/period, must start/end alphanumeric, no consecutive `--` or `..` |
| `version` | no | string | **No pattern enforced.** Spec text: *"Semantic Versioning RECOMMENDED... Clients MUST NOT reject a manifest solely because `version` is not valid Semantic Versioning."* |
| `description` | no | string | — |
| `author` | no | object | `{name?, email?, url?}`, no additional properties |
| `homepage` | no | string | — |
| `repository` | no | string | — |
| `license` | no | string | SPDX recommended, not required |
| `keywords` | no | array\<string\> | — |
| `extensions` | no | object | Client-specific data, keyed by reverse-DNS namespace (e.g. `com.github.copilot`), values are objects with no spec-level schema |

**Root object is `additionalProperties: false`.** An unrecognized top-level
key — e.g. Claude's `displayName`, `defaultEnabled`, `dependencies`,
`settings`, `channels` — is a **hard schema violation** under this family,
not a stripped-and-warn field. A single manifest cannot losslessly serve
both families; grim must emit two distinct files (already the plan's
premise) and must not leak Claude-only keys into the Agent Plugins root,
nor vice versa (the reverse direction is actually safe — Claude tolerates
and strips unknowns).

**Components**: only `skills/` and `mcp.json` are in scope for v1.0. No
`agents`, no `rules`. Client-specific data (Copilot's `agents/`,
`commands/`, `rules/`; Codex's `interface`/`apps`/`hooks`) rides under
`extensions.<reverse-dns>` and its own matching top-level directory
(`com.github.copilot/`, `com.openai/`) — outside the spec's normative
surface entirely. This confirms the task brief's framing: agents are
correctly omitted from the Agent Plugins family, and rules have no home in
either family at all (Claude: explicitly rejected at plugin root; Agent
Plugins: not a defined component).

## Version semantics — build-metadata acceptance

| Family | Semver validated? | Build-metadata-only bump detected as update? |
|---|---|---|
| Claude Code (`claude plugin validate`, install/update loader) | No — "a version string, not checked against semver" | **Yes.** Comparison is plain string (in)equality against the stored `installed_plugins.json` value; any literal string change, including build-metadata-only, is a different string and triggers a new `cache/<marketplace>/<plugin>/<version>/` directory |
| Agent Plugins 1.0 schema | No — schema imposes no pattern on `version`; spec text bars rejecting non-semver strings | Not applicable at the schema layer (the spec defines no comparison algorithm at all — client-specific); Copilot's own comparison algorithm is **undocumented** in primary docs (per `research_plugin_format_compat_a.md`), so "detected as update" cannot be confirmed for Copilot/Codex specifically, only that the string itself is accepted |

**Practical read for grim**: `<v>+<hash>` (e.g. `0.13.0+a1b2c3d`,
`0.0.0+abc123`) is accepted as a valid `version` string by both families'
validators — neither rejects it, and Claude's own loader treats each
distinct hash suffix as a genuinely new version worth re-caching. No
evidence either family's tooling parses out or strips the build-metadata
segment per SemVer §10 precedence rules; both treat `version` as an opaque
string.

## MCP file mapping: grim `McpDescriptor` → Claude `.mcp.json` → Agent Plugins `mcp.json`

Grim's descriptor (`src/oci/mcp.rs`) and its vendor-projection precedent are
already documented in `subsystem-file-structure.md` § MCP servers and
`vendor-capability-watchlist.md`. This table extends that projection to the
two new plugin-file targets.

| grim `McpServer` field | Claude `.mcp.json` (`mcpServers.<name>`) | Agent Plugins `mcp.json` (`mcpServers.<name>`) |
|---|---|---|
| `transport: Stdio` | `command`+`args`(+`type: "stdio"` — required by CC ≥2.1.202 whenever `url` is absent-safe, but a bare `command`-only entry with no `type` is still read as stdio; **best practice: always emit `type`**) | `type: "stdio"` (required key) + `command` |
| `transport: Http` | `type: "http"` + `url` | `type: "streamable-http"` + `url` — **name differs**, not a value copy |
| `transport: Sse` | `type: "sse"` + `url` | `type: "sse"` + `url` (spec explicitly carries `sse` as "deprecated HTTP+SSE transport", same name as Claude) |
| `transport: Ws` | `type: "ws"` + `url` (`wss://`/`ws://`) | **No home.** The Agent Plugins schema recognizes only `stdio`, `streamable-http`, `sse` — no `ws`/websocket type. A `ws` descriptor must be **declined** for this family, same posture grim already takes for OpenCode/Copilot/Codex (`vendor-capability-watchlist.md`) |
| `command` | `command` (stdio only) | `command` (stdio only) — plugin-relative paths must start with `./` per spec text ("plugin-relative paths start with `./`") |
| `args` | `args` (array, substitution valid) | `args` (array, substitution valid in string elements) |
| `env` | `env` (map, substitution valid in **values**) | `env` (map, substitution valid in **values** only — key names never substituted, same as Claude) |
| `url` | `url` (substitution valid, incl. `${VAR:-default}`) | `url` (**no substitution at all** — spec: "No expansion in `command`, `url`, `env` keys, or header names/values") |
| `headers` | `headers` (map, substitution valid in values) | **No home for a substituted header.** Spec: no expansion in header names or values. A literal (non-`${VAR}`) header value still projects; a descriptor whose header carries `${VAR}` must decline this field for the Agent Plugins target |
| `timeout` | `timeout` (ms, integer ≥1000 per CC's own field constraint — grim should clamp/validate before emit) | **Not in the spec's field table found** (stdio: `type`,`command`,`args`,`env`,`cwd`; remote: `type`,`url`,`headers`) — no `timeout` key documented for either server type; decline or drop silently, same posture as other vendors lacking the field |
| `always_load` | `alwaysLoad` (bool, http/sse only) | No documented equivalent — decline |
| `headers_helper` | `headersHelper` (string path/command) | No documented equivalent, and even if grim invented a convention, the field can't safely carry a `${PLUGIN_ROOT}`-relative path since the spec doesn't expand `command`-like fields outside `args`/`env`/`cwd` — decline |
| `cwd` | **No documented top-level `cwd` key** in Claude's own `.mcp.json`/plugin `mcpServers` schema (Claude's `cwd` support, per grim's own vendor doc, is projected for OpenCode/Gemini, not Claude) — decline for Claude, or verify against `code.claude.com/docs/en/mcp` directly before shipping | `cwd` (stdio only, substitution valid — explicitly one of the three fields the spec does expand) |
| `oauth` (`McpOAuth`) | `oauth` object (http/sse only; Claude's own shape: unconfirmed field-for-field against grim's `{client_id, scopes, callback_port, auth_server_metadata_url}` — needs a direct doc pull of `code.claude.com/docs/en/mcp#oauth` before wiring, flagged as a gap below) | **No `oauth` key found** in the fetched Agent Plugins `mcp.json` field table — decline, matching grim's existing decline posture for OpenCode/Copilot/Codex |

**Declared, not silently dropped**: per the task brief and grim's existing
convention (`vendor-capability-watchlist.md`'s "skip + warn" pattern), every
field with "no home" above is a **named decline** at build time for that
target family, not an omission a user discovers by absence.

**Negative — not found in this pass**: the exact Claude `.mcp.json`
`oauth` object's field names (grim assumed `{client_id, scopes,
callback_port, auth_server_metadata_url}` from its own descriptor design,
not verified against a fresh fetch of `code.claude.com/docs/en/mcp`'s OAuth
section in this research pass — the WebFetch summary for that page did not
surface the OAuth object shape). Verify before wiring an OAuth projection
into the Claude plugin `.mcp.json` emitter.

## Per-client component support — corrections to the repo matrix

Confirms `research_plugin_support_matrix.md`'s matrix with three items that
were previously marked unconfirmed/undocumented:

| Question | Answer | Evidence |
|---|---|---|
| Does **Droid** load `agents/` and `.mcp.json` from a Claude-shape plugin? | **Yes**, via an explicit, documented translation layer: `.claude-plugin/` → `.factory-plugin/`, `agents/` → `droids/`, `.mcp.json` → `mcp.json` — not native loading, a conversion step | [docs.factory.ai/cli/configuration/plugins](https://docs.factory.ai/cli/configuration/plugins) |
| Does **Junie** load `agents/` and `.mcp.json` from a Claude-shape plugin? | **Confirmed for marketplace discovery** ("supports both native Junie and Claude plugin formats" for `marketplace.json` listings); **NOT confirmed for component-level fidelity** — no fetched doc states whether Junie reads a Claude plugin's `agents/*.md` or `.mcp.json` content once discovered, only that the marketplace listing format is dual-read. Treat as an open gap, not a yes |
| Does **OpenClaw** load `agents/` and `.mcp.json` from a Claude-shape plugin? | **Yes, both, directly** — "no conversion step required"; its own component-mapping table lists "agents + output styles (Claude)" and "MCP servers (all formats)" as read component types | [docs.openclaw.ai/plugins/bundles](https://docs.openclaw.ai/plugins/bundles) |
| Does **Codex** read Agent Plugins `mcp.json`? | **Yes** — Codex implements the full Agent Plugins 1.0 spec, whose only two components are `skills/` and `mcp.json` | [developers.openai.com/codex/plugins/build](https://developers.openai.com/codex/plugins/build), confirmed in `research_agent_plugins_spec_verify.md` |
| Does **Copilot** read Agent Plugins `mcp.json`? | **Yes** — same spec conformance, GA'd 2026-08-12 across VS Code, Copilot CLI, Copilot app | [github.blog/changelog/2026-08-12-...](https://github.blog/changelog/2026-08-12-agent-plugins-1-0-in-vs-code-copilot-cli-and-the-copilot-app/) |
| Does **Cursor** read Agent Plugins `mcp.json`? | **Yes** — Cursor reads root `plugin.json` with the spec `$schema` (its native `.cursor-plugin/plugin.json` still exists alongside) | [cursor.com/docs/plugins](https://cursor.com/docs/plugins), confirmed in `research_agent_plugins_spec_verify.md` |

## Skill directory naming — decisive answer

- **Claude Code itself**: folder name is the skill's identity by default;
  frontmatter `name` is optional and *overrides* the directory name when
  present (used as the invoked command name). No enforced equality, no
  documented character-pattern regex, no documented max length — only
  convention (lowercase, hyphens; reserved: `synced`, anything starting
  `anthropic-skills:`).
- **claude.ai web upload UI** (a *different* code path from Claude Code's
  plugin loader): does enforce folder-name-must-equal-`name` for a
  standalone skill zip. Unconfirmed whether this extends to a
  plugin-embedded `skills/<name>/SKILL.md` inside an uploaded plugin zip
  (flagged as an open gap in `research_claude_app_install_surfaces.md`,
  unresolved by this pass).
- **Agent Plugins 1.0**: no naming rule at the plugin-manifest layer at
  all — "each immediate child directory containing a file named exactly
  `SKILL.md`... is treated as one skill." Name pattern/length is deferred
  entirely to the separate Agent Skills specification (not fetched in this
  pass; out of scope for the manifest-field question asked).

**Implication for grim's `strip_prefix`-style validation**: do not treat
folder-name/frontmatter-name mismatch as an error condition when building
the export's `skills/` tree for either family — it is accepted by both
loaders. If grim's bundle model already guarantees folder == artifact name
(likely, given `mcp/<name>.toml`'s file-stem convention), this is moot; but
a hand-authored skill with a divergent frontmatter `name` should not be
rejected at export time on cross-vendor-portability grounds — nothing
downstream enforces it.

## negative: not found in this pass

- Claude `.mcp.json`'s exact `oauth` object field names (see MCP mapping
  table note above) — needs a direct fetch of the OAuth section of
  `code.claude.com/docs/en/mcp`.
- Whether Claude Code's `.mcp.json` supports a top-level `cwd` field at
  all (grim's own vendor table projects `cwd` for OpenCode/Gemini only,
  never Claude — this pass did not independently re-verify that Claude
  lacks it, only that the fetched `.mcp.json` field table omitted it).
- Agent Plugins 1.0's `mcp.json` `timeout` field — absent from the fetched
  field table; not verified as "does not exist" vs. "not documented in the
  summarized extract."
- The Agent Skills specification's own name-pattern/length rule (deferred
  by Agent Plugins 1.0 to that sibling spec, not fetched here — scope was
  the two plugin-manifest families, not the skill-payload spec, which
  `research_plugin_format_compat_b.md`/`_c.md` already partially cover).

## Sources

| Source | Type | Date | Relevance |
|--------|------|------|-----------|
| [code.claude.com/docs/en/plugins-reference](https://code.claude.com/docs/en/plugins-reference) | Docs (primary, Anthropic) | fetched 2026-09-27 | Full `plugin.json` field table, `name` validation rule, `version` semantics, standard layout, `${CLAUDE_PLUGIN_ROOT}` etc. variable table with per-field resolution scope |
| [code.claude.com/docs/en/mcp](https://code.claude.com/docs/en/mcp) | Docs (primary, Anthropic) | fetched 2026-09-27 | `.mcp.json` top-level key, per-server field table, transport-type determination, full variable-substitution syntax and credential-blocklist behavior |
| [code.claude.com/docs/en/skills](https://code.claude.com/docs/en/skills) | Docs (primary, Anthropic) | fetched 2026-09-27 | Skill folder-vs-frontmatter-name rule (name defaults to directory, not required to match) |
| [agent-plugins.org/specification](https://agent-plugins.org/specification) | Spec (primary) | fetched 2026-09-27 | Full `plugin.json` field table, `name` pattern, `version` non-semver-rejection clause, `mcp.json` schema and field-level substitution scope, skills/ discovery rule |
| [agent-plugins.org/schemas/1.0.0/plugin.schema.json](https://agent-plugins.org/schemas/1.0.0/plugin.schema.json) | JSON Schema (primary) | fetched 2026-09-27 | Confirms `additionalProperties: false` at root, exact `name` regex, `author` sub-object shape |
| `.agents/research/research_plugin_format_compat_a.md` | Prior research (this repo) | 2026-09-27 | Claude/Copilot/Codex version-comparison algorithms, cross-read matrix |
| `.agents/research/research_plugin_format_compat_c.md` | Prior research (this repo) | 2026-09-27 | Junie/OpenClaw/Antigravity component-mapping tables |
| `.agents/research/research_agent_plugins_spec_verify.md` | Prior research (this repo) | 2026-09-27 | Spec provenance, per-part verdicts on Copilot/Codex/Cursor conformance |
| `src/oci/mcp.rs` | Source (this repo) | read 2026-09-27 | grim's canonical `McpDescriptor`/`McpServer` field set for the mapping table |
| `docs/src/content/docs/mcp-servers.md` | Docs (this repo) | read 2026-09-27 | grim's own per-field vendor-projection convention (common fields, server table) |
